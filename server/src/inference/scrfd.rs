//! SCRFD face detector post-processing (M2 turn 23).
//!
//! Reference: insightface SCRFD (`buffalo_s/det_500m.onnx`). The model is
//! exported with dynamic input H/W; we always run it at a fixed [`INPUT_SIZE`]
//! letterbox so the anchor grid is constant.
//!
//! Output layout (9 tensors, grouped per stride [8, 16, 32]):
//!
//! | level | score shape | bbox shape | kps shape | anchors |
//! |-------|-------------|------------|-----------|---------|
//! | s=8   | (N, 1)      | (N, 4)     | (N, 10)   | 12800   |
//! | s=16  | (N, 1)      | (N, 4)     | (N, 10)   | 3200    |
//! | s=32  | (N, 1)      | (N, 4)     | (N, 10)   | 800     |
//!
//! Where `N = (input/stride)^2 * NUM_ANCHORS`. For each anchor at grid cell
//! `(gx, gy)` with stride `s`, the anchor centre is `(gx*s, gy*s)` (pixel
//! coordinates inside the letterboxed canvas) repeated `NUM_ANCHORS` times.
//!
//! Bbox decoding: bbox values are unbounded floats representing
//! `(left, top, right, bottom)` distances **in stride units**, so
//! `x1 = cx - left*s`, `x2 = cx + right*s`, etc.
//
// We perform standard greedy IoU NMS at [`NMS_IOU`] across all kept anchors.

use anyhow::{anyhow, Result};
use ndarray::Array4;

use super::preprocess::{decode_letterbox_nchw, Letterbox, Norm};

/// Square input edge for SCRFD inference. The buffalo_s `det_500m.onnx` was
/// exported with dynamic H/W; we always run at 640 to match the reference
/// configuration (which is also what the anchor strides assume).
pub const INPUT_SIZE: u32 = 640;

/// SCRFD models in the buffalo_s family use 2 anchors per grid cell.
pub const NUM_ANCHORS: usize = 2;

/// FPN strides in output order (s=8, 16, 32).
pub const STRIDES: [u32; 3] = [8, 16, 32];

/// Score threshold (sigmoid space). Reference SCRFD inference uses 0.5; we
/// keep it the same so behaviour matches the upstream Python pipeline.
pub const SCORE_THRESHOLD: f32 = 0.5;

/// IoU threshold for greedy NMS.
pub const NMS_IOU: f32 = 0.4;

/// One face detection in **original image** pixel coordinates.
#[derive(Debug, Clone)]
pub struct FaceDetection {
    pub bbox: (f32, f32, f32, f32),
    pub score: f32,
    /// 5 landmarks (left eye, right eye, nose, left mouth, right mouth) in
    /// **original image** pixel coordinates.
    pub kps: [(f32, f32); 5],
}

/// One raw candidate before NMS, in **letterboxed** canvas coordinates.
#[derive(Debug, Clone, Copy)]
struct RawCandidate {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    score: f32,
    kps: [(f32, f32); 5],
}

/// Decode + normalize an image at `path` into the SCRFD input tensor and
/// return the letterbox geometry / source dimensions for unprojection.
pub fn preprocess_image(path: &std::path::Path) -> Result<(Array4<f32>, Letterbox, (u32, u32))> {
    decode_letterbox_nchw(path, INPUT_SIZE, Norm::Scrfd)
}

/// Decode the 9 raw SCRFD output tensors into post-NMS face detections in
/// the original image's pixel space.
///
/// Each input slice is the row-major flat tensor for one `(level, head)`
/// pair: `scores[level]` is `(N, 1)`, `bboxes[level]` is `(N, 4)`, and
/// `kps[level]` is `(N, 10)` where `N = (INPUT_SIZE/stride)^2 * NUM_ANCHORS`.
pub fn decode_outputs(
    scores: [&[f32]; 3],
    bboxes: [&[f32]; 3],
    kps: [&[f32]; 3],
    lb: Letterbox,
    src_w: u32,
    src_h: u32,
) -> Result<Vec<FaceDetection>> {
    let mut raw: Vec<RawCandidate> = Vec::new();
    for (level_idx, &stride) in STRIDES.iter().enumerate() {
        decode_level(
            scores[level_idx],
            bboxes[level_idx],
            kps[level_idx],
            stride,
            &mut raw,
        )?;
    }

    // Greedy NMS in letterbox space. IoU is invariant under the linear
    // unprojection so doing it here is equivalent and cheaper.
    raw.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let kept = nms(&raw, NMS_IOU);

    // Unproject letterbox → original image pixels.
    let out: Vec<FaceDetection> = kept
        .into_iter()
        .map(|c| {
            let (x1, y1, x2, y2) = lb.unproject(c.x1, c.y1, c.x2, c.y2, src_w, src_h);
            let mut kps_out = [(0.0f32, 0.0f32); 5];
            for (i, (kx, ky)) in c.kps.iter().enumerate() {
                let s = lb.scale.max(1e-6);
                let ox = (*kx - lb.pad_x as f32) / s;
                let oy = (*ky - lb.pad_y as f32) / s;
                kps_out[i] = (ox.clamp(0.0, src_w as f32), oy.clamp(0.0, src_h as f32));
            }
            FaceDetection {
                bbox: (x1, y1, x2, y2),
                score: c.score,
                kps: kps_out,
            }
        })
        .collect();
    Ok(out)
}

fn decode_level(
    scores: &[f32],
    bboxes: &[f32],
    kps: &[f32],
    stride: u32,
    out: &mut Vec<RawCandidate>,
) -> Result<()> {
    let feat = (INPUT_SIZE / stride) as usize;
    let n = feat * feat * NUM_ANCHORS;
    if scores.len() != n {
        return Err(anyhow!(
            "SCRFD scores level stride={stride}: expected {n} got {}",
            scores.len()
        ));
    }
    if bboxes.len() != n * 4 {
        return Err(anyhow!(
            "SCRFD bboxes level stride={stride}: expected {} got {}",
            n * 4,
            bboxes.len()
        ));
    }
    if kps.len() != n * 10 {
        return Err(anyhow!(
            "SCRFD kps level stride={stride}: expected {} got {}",
            n * 10,
            kps.len()
        ));
    }
    let s_f = stride as f32;
    for gy in 0..feat {
        for gx in 0..feat {
            for a in 0..NUM_ANCHORS {
                let idx = (gy * feat + gx) * NUM_ANCHORS + a;
                let score = scores[idx];
                if score < SCORE_THRESHOLD {
                    continue;
                }
                let cx = gx as f32 * s_f;
                let cy = gy as f32 * s_f;
                let bb = &bboxes[idx * 4..idx * 4 + 4];
                let x1 = cx - bb[0] * s_f;
                let y1 = cy - bb[1] * s_f;
                let x2 = cx + bb[2] * s_f;
                let y2 = cy + bb[3] * s_f;
                let kp_off = idx * 10;
                let mut kps_arr = [(0.0f32, 0.0f32); 5];
                for k in 0..5 {
                    let dx = kps[kp_off + k * 2];
                    let dy = kps[kp_off + k * 2 + 1];
                    kps_arr[k] = (cx + dx * s_f, cy + dy * s_f);
                }
                out.push(RawCandidate {
                    x1,
                    y1,
                    x2,
                    y2,
                    score,
                    kps: kps_arr,
                });
            }
        }
    }
    Ok(())
}

fn iou(a: &RawCandidate, b: &RawCandidate) -> f32 {
    let ix1 = a.x1.max(b.x1);
    let iy1 = a.y1.max(b.y1);
    let ix2 = a.x2.min(b.x2);
    let iy2 = a.y2.min(b.y2);
    let iw = (ix2 - ix1).max(0.0);
    let ih = (iy2 - iy1).max(0.0);
    let inter = iw * ih;
    let area_a = (a.x2 - a.x1).max(0.0) * (a.y2 - a.y1).max(0.0);
    let area_b = (b.x2 - b.x1).max(0.0) * (b.y2 - b.y1).max(0.0);
    let union = area_a + area_b - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

fn nms(sorted: &[RawCandidate], iou_thresh: f32) -> Vec<RawCandidate> {
    let mut keep: Vec<RawCandidate> = Vec::new();
    for cand in sorted {
        let mut suppress = false;
        for k in &keep {
            if iou(cand, k) > iou_thresh {
                suppress = true;
                break;
            }
        }
        if !suppress {
            keep.push(*cand);
        }
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::preprocess::Letterbox;

    fn empty_lb() -> Letterbox {
        Letterbox {
            scale: 1.0,
            pad_x: 0,
            pad_y: 0,
            out_w: INPUT_SIZE,
            out_h: INPUT_SIZE,
        }
    }

    fn alloc_zero_outputs() -> (
        Vec<f32>,
        Vec<f32>,
        Vec<f32>,
        Vec<f32>,
        Vec<f32>,
        Vec<f32>,
        Vec<f32>,
        Vec<f32>,
        Vec<f32>,
    ) {
        // 3 levels: stride 8 → 80*80*2 = 12800; stride 16 → 3200; stride 32 → 800.
        let n8 = 80 * 80 * NUM_ANCHORS;
        let n16 = 40 * 40 * NUM_ANCHORS;
        let n32 = 20 * 20 * NUM_ANCHORS;
        (
            vec![0.0; n8],
            vec![0.0; n8 * 4],
            vec![0.0; n8 * 10],
            vec![0.0; n16],
            vec![0.0; n16 * 4],
            vec![0.0; n16 * 10],
            vec![0.0; n32],
            vec![0.0; n32 * 4],
            vec![0.0; n32 * 10],
        )
    }

    #[test]
    fn no_scores_returns_empty() {
        let (s8, b8, k8, s16, b16, k16, s32, b32, k32) = alloc_zero_outputs();
        let dets = decode_outputs(
            [&s8, &s16, &s32],
            [&b8, &b16, &b32],
            [&k8, &k16, &k32],
            empty_lb(),
            INPUT_SIZE,
            INPUT_SIZE,
        )
        .expect("decode must succeed on zero outputs");
        assert_eq!(dets.len(), 0);
    }

    #[test]
    fn single_anchor_decodes_to_centred_box() {
        let (mut s8, mut b8, k8, s16, b16, k16, s32, b32, k32) = alloc_zero_outputs();
        // Place one strong score at stride-8 cell (gx=10, gy=10), anchor 0.
        // Anchor centre = (80, 80) px.
        let gx = 10usize;
        let gy = 10usize;
        let idx = (gy * 80 + gx) * NUM_ANCHORS;
        s8[idx] = 0.9;
        // Predict a 32×32 box centred on the anchor: l=t=r=b=2 stride units = 16 px.
        b8[idx * 4] = 2.0;
        b8[idx * 4 + 1] = 2.0;
        b8[idx * 4 + 2] = 2.0;
        b8[idx * 4 + 3] = 2.0;
        let dets = decode_outputs(
            [&s8, &s16, &s32],
            [&b8, &b16, &b32],
            [&k8, &k16, &k32],
            empty_lb(),
            INPUT_SIZE,
            INPUT_SIZE,
        )
        .expect("decode");
        assert_eq!(dets.len(), 1);
        let d = &dets[0];
        assert!((d.score - 0.9).abs() < 1e-5);
        assert!((d.bbox.0 - 64.0).abs() < 1e-3);
        assert!((d.bbox.1 - 64.0).abs() < 1e-3);
        assert!((d.bbox.2 - 96.0).abs() < 1e-3);
        assert!((d.bbox.3 - 96.0).abs() < 1e-3);
    }

    #[test]
    fn nms_suppresses_overlapping_boxes() {
        let (mut s8, mut b8, k8, s16, b16, k16, s32, b32, k32) = alloc_zero_outputs();
        // Two adjacent anchors with strong scores predicting almost the same
        // box: NMS should keep only the higher-scoring one.
        let a = (10usize * 80 + 10) * NUM_ANCHORS;
        let b = (10usize * 80 + 10) * NUM_ANCHORS + 1;
        s8[a] = 0.95;
        s8[b] = 0.90;
        for &i in &[a, b] {
            b8[i * 4] = 4.0;
            b8[i * 4 + 1] = 4.0;
            b8[i * 4 + 2] = 4.0;
            b8[i * 4 + 3] = 4.0;
        }
        let dets = decode_outputs(
            [&s8, &s16, &s32],
            [&b8, &b16, &b32],
            [&k8, &k16, &k32],
            empty_lb(),
            INPUT_SIZE,
            INPUT_SIZE,
        )
        .expect("decode");
        assert_eq!(dets.len(), 1);
        assert!((dets[0].score - 0.95).abs() < 1e-5);
    }

    #[test]
    fn letterbox_unprojects_to_original_coords() {
        let (mut s8, mut b8, k8, s16, b16, k16, s32, b32, k32) = alloc_zero_outputs();
        let idx = (10usize * 80 + 10) * NUM_ANCHORS;
        s8[idx] = 0.9;
        b8[idx * 4] = 2.0;
        b8[idx * 4 + 1] = 2.0;
        b8[idx * 4 + 2] = 2.0;
        b8[idx * 4 + 3] = 2.0;
        // Letterbox: scale 0.5, no pad — so original image is 1280×1280.
        let lb = Letterbox {
            scale: 0.5,
            pad_x: 0,
            pad_y: 0,
            out_w: INPUT_SIZE,
            out_h: INPUT_SIZE,
        };
        let dets = decode_outputs(
            [&s8, &s16, &s32],
            [&b8, &b16, &b32],
            [&k8, &k16, &k32],
            lb,
            1280,
            1280,
        )
        .expect("decode");
        assert_eq!(dets.len(), 1);
        let d = &dets[0];
        // Letterbox box was 64..96; unproject scale=0.5 → 128..192.
        assert!((d.bbox.0 - 128.0).abs() < 1e-3);
        assert!((d.bbox.2 - 192.0).abs() < 1e-3);
    }
}
