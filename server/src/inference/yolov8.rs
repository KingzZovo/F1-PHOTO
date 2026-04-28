//! YOLOv8 output decode + NMS (Step B; shape-tolerant since #7b-prep).
//!
//! Input: ultralytics-exported YOLOv8 ONNX with input `images (1,3,640,640)`
//! and a single output `output0` of shape `(1, 4 + NC, 8400)`. The `4 + NC`
//! channels are laid out as `[cx, cy, w, h, c0, c1, ..., c_{NC-1}]` where
//! `c_i` are post-sigmoid class probabilities for the model's `NC` classes.
//! Coordinates are in pixel space at the model's 640x640 letterbox canvas.
//!
//! 8400 = 80*80 + 40*40 + 20*20 anchors (P3+P4+P5 feature pyramid). The
//! export already includes the DFL+sigmoid+box-decode head so we can read
//! `(cx,cy,w,h)` directly without anchor math; we just need confidence
//! filtering + NMS.
//!
//! `NC` is **inferred at runtime** from the output buffer length
//! (`out0.len() / NUM_ANCHORS - 4`) so the same decoder accepts both:
//!
//! 1. The shipped baseline `models/object_detect.onnx` — YOLOv8n trained on
//!    COCO, `NC = 80` (`output0` shape `(1, 84, 8400)`).
//! 2. Retrained candidates produced by milestone #7b that fine-tune on the
//!    operator-corrections dataset materialised by #7a. Per the #5-skel
//!    scope revision the candidate is a single-class detector
//!    (`NC = 1`, class 0 = `tool`), giving `output0` shape `(1, 5, 8400)`.
//!
//! Important caveat (now narrowed by #7b/#7c): the COCO baseline's class
//! labels are unrelated to F1-photo's `tools`/`devices` taxonomy. The
//! object pipeline uses YOLOv8 only to localise *something object-like*
//! in the frame and then runs DINOv2 over each crop to produce a
//! re-identification embedding against the project's tool/device gallery.
//! After the first #7c promote the detector becomes tool-specific
//! (`NC = 1`) and `det.class_id` is always `0`; the DINOv2 re-id stage is
//! unchanged.

use anyhow::{anyhow, Result};

use super::preprocess::Letterbox;

/// YOLOv8 letterbox input size used at export time (`imgsz=640`).
pub const INPUT_SIZE: u32 = 640;

/// COCO baseline channel count (`4 + LEGACY_COCO_NUM_CLASSES`). Retained as a
/// public constant so the test suite can build a synthetic `output0` of the
/// shipped baseline shape without re-deriving it; runtime decode uses the
/// inferred channel count, not this constant.
pub const LEGACY_COCO_NUM_CHANNELS: usize = 84;
/// COCO baseline class count for the shipped `models/object_detect.onnx`.
/// Runtime decode infers the candidate model's class count from the output
/// buffer length; this constant is only used by tests + back-compat docs.
pub const LEGACY_COCO_NUM_CLASSES: usize = 80;

/// Total number of anchors for `imgsz=640` (P3 80x80 + P4 40x40 + P5 20x20).
pub const NUM_ANCHORS: usize = 8400;

/// Default confidence threshold for keeping a candidate. Matches the
/// ultralytics CLI default. Tunable via [`decode_outputs`].
pub const DEFAULT_CONF: f32 = 0.25;

/// Default IoU threshold for NMS suppression. Matches ultralytics default.
pub const DEFAULT_IOU: f32 = 0.45;

/// Hard cap on detections returned after NMS. Keeps DB writes bounded; in
/// practice typical photos rarely have >10 distinct objects worth
/// re-identifying against the tool/device gallery.
pub const MAX_DETECTIONS: usize = 10;

/// One YOLOv8 detection mapped back to the original image's pixel space.
#[derive(Debug, Clone, Copy)]
pub struct ObjectDet {
    /// `(x1, y1, x2, y2)` in **original-image** coordinates (post-unproject).
    pub bbox: (f32, f32, f32, f32),
    /// Best per-anchor class confidence (post-sigmoid, in `[0, 1]`).
    pub score: f32,
    /// Best class index in `0..80` (COCO label space).
    pub class_id: usize,
}

/// Decode YOLOv8 `output0` (shape `[1, 4 + NC, NUM_ANCHORS]`, channel-major
/// contiguous) into a list of [`ObjectDet`]s in the original image's
/// coordinate space. `NC` (the number of classes) is inferred at runtime
/// from `out0.len() / NUM_ANCHORS - 4`, so the same decoder works for the
/// COCO baseline (`NC = 80`) and for the single-class retrained candidate
/// produced by milestones #7a/#7b (`NC = 1`).
///
/// Steps:
/// 1. For each of `NUM_ANCHORS` anchors, find the max-class score across
///    channels `4..(4 + NC)`.
/// 2. Drop anchors with score `< conf`.
/// 3. Decode `(cx, cy, w, h)` from channels `0..4` to letterbox-space xyxy.
/// 4. Unproject through the letterbox geometry to original-image xyxy.
/// 5. Class-agnostic NMS at `iou` IoU.
/// 6. Return at most `MAX_DETECTIONS`, sorted by score descending.
///
/// Errors if `out0.len()` is not a positive multiple of [`NUM_ANCHORS`] or
/// if the inferred `NC` is `< 1` (i.e. fewer than 5 channels).
pub fn decode_outputs(
    out0: &[f32],
    lb: Letterbox,
    src_w: u32,
    src_h: u32,
    conf: f32,
    iou: f32,
) -> Result<Vec<ObjectDet>> {
    if out0.is_empty() || out0.len() % NUM_ANCHORS != 0 {
        return Err(anyhow!(
            "YOLOv8 output0 expected a positive multiple of {} floats; got {}",
            NUM_ANCHORS,
            out0.len()
        ));
    }
    let num_channels = out0.len() / NUM_ANCHORS;
    if num_channels < 5 {
        return Err(anyhow!(
            "YOLOv8 output0 expected ≥ 5 channels (4 box + ≥ 1 class); got {}",
            num_channels
        ));
    }
    let num_classes = num_channels - 4;

    // Channel-major flat layout: out0[c * NUM_ANCHORS + a].
    let ch = |c: usize, a: usize| -> f32 { out0[c * NUM_ANCHORS + a] };

    let mut candidates: Vec<ObjectDet> = Vec::new();
    for a in 0..NUM_ANCHORS {
        // Best class score across the model's NC class channels.
        let mut best_score = 0.0f32;
        let mut best_class = 0usize;
        for c in 0..num_classes {
            let s = ch(4 + c, a);
            if s > best_score {
                best_score = s;
                best_class = c;
            }
        }
        if best_score < conf {
            continue;
        }

        let cx = ch(0, a);
        let cy = ch(1, a);
        let w = ch(2, a);
        let h = ch(3, a);
        let bx1 = cx - w * 0.5;
        let by1 = cy - h * 0.5;
        let bx2 = cx + w * 0.5;
        let by2 = cy + h * 0.5;
        let (x1, y1, x2, y2) = lb.unproject(bx1, by1, bx2, by2, src_w, src_h);
        // Reject degenerate boxes (post-unproject zero area).
        if x2 <= x1 + 1.0 || y2 <= y1 + 1.0 {
            continue;
        }
        candidates.push(ObjectDet {
            bbox: (x1, y1, x2, y2),
            score: best_score,
            class_id: best_class,
        });
    }

    // Class-agnostic NMS by score descending.
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut kept: Vec<ObjectDet> = Vec::with_capacity(candidates.len().min(MAX_DETECTIONS));
    for cand in candidates {
        if kept.len() >= MAX_DETECTIONS {
            break;
        }
        let mut suppressed = false;
        for k in &kept {
            if iou_xyxy(cand.bbox, k.bbox) >= iou {
                suppressed = true;
                break;
            }
        }
        if !suppressed {
            kept.push(cand);
        }
    }
    Ok(kept)
}

/// IoU between two `(x1, y1, x2, y2)` boxes. `0` for non-overlapping or
/// degenerate boxes.
fn iou_xyxy(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> f32 {
    let (ax1, ay1, ax2, ay2) = a;
    let (bx1, by1, bx2, by2) = b;
    let ix1 = ax1.max(bx1);
    let iy1 = ay1.max(by1);
    let ix2 = ax2.min(bx2);
    let iy2 = ay2.min(by2);
    let iw = (ix2 - ix1).max(0.0);
    let ih = (iy2 - iy1).max(0.0);
    let inter = iw * ih;
    let area_a = ((ax2 - ax1).max(0.0)) * ((ay2 - ay1).max(0.0));
    let area_b = ((bx2 - bx1).max(0.0)) * ((by2 - by1).max(0.0));
    let denom = area_a + area_b - inter;
    if denom <= 0.0 {
        0.0
    } else {
        inter / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_lb() -> Letterbox {
        Letterbox {
            scale: 1.0,
            pad_x: 0,
            pad_y: 0,
            out_w: INPUT_SIZE,
            out_h: INPUT_SIZE,
        }
    }

    /// Build a synthetic `output0` with a single anchor having a strong
    /// score. `num_classes` controls the channel count (use
    /// [`LEGACY_COCO_NUM_CLASSES`] for the COCO baseline shape, `1` for
    /// the single-class retrained candidate produced by #7b).
    #[allow(clippy::too_many_arguments)]
    fn synth_one_anchor(
        num_classes: usize,
        anchor_idx: usize,
        cx: f32,
        cy: f32,
        w: f32,
        h: f32,
        class_id: usize,
        score: f32,
    ) -> Vec<f32> {
        assert!(class_id < num_classes, "class_id out of range");
        let num_channels = 4 + num_classes;
        let mut buf = vec![0.0f32; num_channels * NUM_ANCHORS];
        buf[anchor_idx] = cx;
        buf[NUM_ANCHORS + anchor_idx] = cy;
        buf[2 * NUM_ANCHORS + anchor_idx] = w;
        buf[3 * NUM_ANCHORS + anchor_idx] = h;
        buf[(4 + class_id) * NUM_ANCHORS + anchor_idx] = score;
        buf
    }

    #[test]
    fn rejects_wrong_size_input() {
        let lb = empty_lb();
        let result = decode_outputs(&[0.0; 10], lb, 640, 640, DEFAULT_CONF, DEFAULT_IOU);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_too_few_channels() {
        // 4 box channels but zero class channels → invalid.
        let buf = vec![0.0f32; 4 * NUM_ANCHORS];
        let lb = empty_lb();
        let result = decode_outputs(&buf, lb, 640, 640, DEFAULT_CONF, DEFAULT_IOU);
        assert!(
            result.is_err(),
            "4 channels (zero classes) must be rejected"
        );
    }

    #[test]
    fn single_high_score_anchor_decodes_to_centre_box() {
        let buf = synth_one_anchor(
            LEGACY_COCO_NUM_CLASSES,
            123,
            320.0,
            320.0,
            100.0,
            80.0,
            7,
            0.9,
        );
        let lb = empty_lb();
        let dets = decode_outputs(&buf, lb, 640, 640, DEFAULT_CONF, DEFAULT_IOU).unwrap();
        assert_eq!(dets.len(), 1);
        let d = dets[0];
        assert_eq!(d.class_id, 7);
        assert!((d.score - 0.9).abs() < 1e-6);
        assert!((d.bbox.0 - 270.0).abs() < 1e-3);
        assert!((d.bbox.1 - 280.0).abs() < 1e-3);
        assert!((d.bbox.2 - 370.0).abs() < 1e-3);
        assert!((d.bbox.3 - 360.0).abs() < 1e-3);
    }

    #[test]
    fn single_class_candidate_decodes_to_class_zero() {
        // `(1, 5, 8400)` shape produced by the #7b retrained candidate. The
        // decoder must accept it without panicking and report `class_id = 0`
        // for every detection regardless of how many anchors fire.
        let buf = synth_one_anchor(1, 42, 200.0, 240.0, 60.0, 60.0, 0, 0.8);
        assert_eq!(buf.len(), 5 * NUM_ANCHORS, "expected (4 + 1) * NUM_ANCHORS");
        let lb = empty_lb();
        let dets = decode_outputs(&buf, lb, 640, 640, DEFAULT_CONF, DEFAULT_IOU).unwrap();
        assert_eq!(dets.len(), 1);
        assert_eq!(dets[0].class_id, 0);
        assert!((dets[0].score - 0.8).abs() < 1e-6);
    }

    #[test]
    fn three_class_candidate_picks_highest_score() {
        // Mid-cycle candidate with `NC = 3` (e.g. `tool` + 2 distractor
        // classes if a future architecture decision adds them). Anchor has
        // a stronger score on class 2 than class 0 — the decoder should
        // pick class 2.
        let mut buf = synth_one_anchor(3, 0, 100.0, 120.0, 50.0, 50.0, 0, 0.4);
        // Overwrite class 2's channel for anchor 0 with a higher score.
        // num_channels = 4 + 3 = 7; class 2 lives at channel index 4 + 2 = 6.
        buf[6 * NUM_ANCHORS] = 0.7;
        let lb = empty_lb();
        let dets = decode_outputs(&buf, lb, 640, 640, DEFAULT_CONF, DEFAULT_IOU).unwrap();
        assert_eq!(dets.len(), 1);
        assert_eq!(dets[0].class_id, 2);
        assert!((dets[0].score - 0.7).abs() < 1e-6);
    }

    #[test]
    fn below_threshold_anchors_are_dropped() {
        let buf = synth_one_anchor(
            LEGACY_COCO_NUM_CLASSES,
            0,
            100.0,
            100.0,
            50.0,
            50.0,
            0,
            DEFAULT_CONF * 0.5,
        );
        let lb = empty_lb();
        let dets = decode_outputs(&buf, lb, 640, 640, DEFAULT_CONF, DEFAULT_IOU).unwrap();
        assert!(dets.is_empty());
    }

    #[test]
    fn nms_suppresses_overlapping_lower_score() {
        let mut buf = vec![0.0f32; LEGACY_COCO_NUM_CHANNELS * NUM_ANCHORS];
        // Anchor 0: high-score (0.9), large box centred at (320, 320).
        buf[0] = 320.0;
        buf[NUM_ANCHORS] = 320.0;
        buf[2 * NUM_ANCHORS] = 200.0;
        buf[3 * NUM_ANCHORS] = 200.0;
        buf[(4 + 3) * NUM_ANCHORS] = 0.9;
        // Anchor 1: lower score (0.6), nearly identical box (heavy overlap).
        buf[1] = 322.0;
        buf[NUM_ANCHORS + 1] = 318.0;
        buf[2 * NUM_ANCHORS + 1] = 198.0;
        buf[3 * NUM_ANCHORS + 1] = 198.0;
        buf[(4 + 3) * NUM_ANCHORS + 1] = 0.6;

        let lb = empty_lb();
        let dets = decode_outputs(&buf, lb, 640, 640, DEFAULT_CONF, DEFAULT_IOU).unwrap();
        assert_eq!(dets.len(), 1, "NMS should suppress the lower-score overlap");
        assert!((dets[0].score - 0.9).abs() < 1e-6);
    }

    #[test]
    fn iou_xyxy_disjoint_is_zero() {
        let a = (0.0, 0.0, 10.0, 10.0);
        let b = (20.0, 20.0, 30.0, 30.0);
        assert!(iou_xyxy(a, b).abs() < 1e-6);
    }

    #[test]
    fn iou_xyxy_identical_is_one() {
        let a = (0.0, 0.0, 10.0, 10.0);
        assert!((iou_xyxy(a, a) - 1.0).abs() < 1e-6);
    }
}
