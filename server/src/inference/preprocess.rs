//! Image preprocessing helpers for the inference pipeline (M2 turn 10).
//!
//! Each model in the registry has a fixed input shape:
//!
//! | Model        | input shape | normalization                |
//! |--------------|-------------|------------------------------|
//! | SCRFD        | 1x3x640x640 | (px - 127.5) / 128.0         |
//! | YOLOv8n      | 1x3x640x640 | px / 255.0                   |
//! | ArcFace      | 1x3x112x112 | (px - 127.5) / 127.5         |
//! | DINOv2-small | 1x3x224x224 | imagenet mean/std            |
//!
//! All helpers return `ndarray::Array4<f32>` (NCHW) so they can be fed straight
//! into ort sessions. Letterbox helpers preserve aspect ratio with gray (114)
//! padding, matching the reference SCRFD / YOLOv8 preprocess used by the
//! reference Python implementations.

use anyhow::Result;
use image::{DynamicImage, GenericImageView, RgbImage, imageops::FilterType};
use ndarray::Array4;

/// Normalization preset.
#[derive(Debug, Clone, Copy)]
pub enum Norm {
    /// `(px - 127.5) / 128.0`. Used by SCRFD.
    Scrfd,
    /// `(px - 127.5) / 127.5`. Used by ArcFace / MobileFaceNet.
    ArcFace,
    /// `px / 255.0`. Used by YOLOv8.
    Unit,
    /// `(px/255 - mean) / std`, ImageNet. Used by DINOv2.
    ImageNet,
}

/// Result of a letterbox resize: the resized canvas and the geometric
/// parameters needed to map detections back to the original image.
#[derive(Debug, Clone, Copy)]
pub struct Letterbox {
    pub scale: f32,
    pub pad_x: u32,
    pub pad_y: u32,
    pub out_w: u32,
    pub out_h: u32,
}

impl Letterbox {
    /// Map a detection bbox in the model's coordinate space back to the
    /// original image. Returns `(x1, y1, x2, y2)` clamped to `[0, src_w/h]`.
    pub fn unproject(&self, bx1: f32, by1: f32, bx2: f32, by2: f32, src_w: u32, src_h: u32) -> (f32, f32, f32, f32) {
        let s = self.scale.max(1e-6);
        let x1 = (bx1 - self.pad_x as f32) / s;
        let y1 = (by1 - self.pad_y as f32) / s;
        let x2 = (bx2 - self.pad_x as f32) / s;
        let y2 = (by2 - self.pad_y as f32) / s;
        (
            x1.clamp(0.0, src_w as f32),
            y1.clamp(0.0, src_h as f32),
            x2.clamp(0.0, src_w as f32),
            y2.clamp(0.0, src_h as f32),
        )
    }
}

/// Decode a JPEG/PNG image from disk.
pub fn decode_path(path: &std::path::Path) -> Result<DynamicImage> {
    let bytes = std::fs::read(path)?;
    let img = image::load_from_memory(&bytes)?;
    Ok(img)
}

/// Letterbox resize an RGB image into a `dst x dst` canvas with gray padding.
pub fn letterbox(src: &DynamicImage, dst: u32) -> (RgbImage, Letterbox) {
    let (sw, sh) = src.dimensions();
    let scale = (dst as f32 / sw as f32).min(dst as f32 / sh as f32);
    let nw = ((sw as f32) * scale).round().max(1.0) as u32;
    let nh = ((sh as f32) * scale).round().max(1.0) as u32;
    let resized = src.resize_exact(nw, nh, FilterType::Triangle).to_rgb8();
    let pad_x = (dst - nw) / 2;
    let pad_y = (dst - nh) / 2;
    let mut canvas = RgbImage::from_pixel(dst, dst, image::Rgb([114, 114, 114]));
    image::imageops::overlay(
        &mut canvas,
        &resized,
        pad_x as i64,
        pad_y as i64,
    );
    (
        canvas,
        Letterbox { scale, pad_x, pad_y, out_w: dst, out_h: dst },
    )
}

/// Convert an RGB canvas into NCHW `Array4<f32>` with the given normalization.
pub fn to_nchw(img: &RgbImage, norm: Norm) -> Array4<f32> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut arr = Array4::<f32>::zeros((1, 3, h, w));
    let mean = [0.485f32, 0.456, 0.406];
    let std = [0.229f32, 0.224, 0.225];
    for (x, y, p) in img.enumerate_pixels() {
        let (xu, yu) = (x as usize, y as usize);
        for c in 0..3 {
            let raw = p.0[c] as f32;
            let v = match norm {
                Norm::Scrfd => (raw - 127.5) / 128.0,
                Norm::ArcFace => (raw - 127.5) / 127.5,
                Norm::Unit => raw / 255.0,
                Norm::ImageNet => (raw / 255.0 - mean[c]) / std[c],
            };
            arr[[0, c, yu, xu]] = v;
        }
    }
    arr
}

/// One-shot: decode + letterbox + normalize. Common for SCRFD / YOLOv8.
pub fn decode_letterbox_nchw(
    path: &std::path::Path,
    dst: u32,
    norm: Norm,
) -> Result<(Array4<f32>, Letterbox, (u32, u32))> {
    let img = decode_path(path)?;
    let (sw, sh) = img.dimensions();
    let (canvas, lb) = letterbox(&img, dst);
    let nchw = to_nchw(&canvas, norm);
    Ok((nchw, lb, (sw, sh)))
}

/// Crop `(x1,y1,x2,y2)` from the original image (any orientation) and resize
/// to `dst x dst` (no letterbox; ArcFace / DINOv2 expect a tight square).
/// Coordinates are clamped to image bounds; degenerate boxes get a 1x1 crop.
pub fn crop_resize(
    src: &DynamicImage,
    bbox: (f32, f32, f32, f32),
    dst: u32,
) -> RgbImage {
    let (sw, sh) = src.dimensions();
    let (x1, y1, x2, y2) = bbox;
    let x1 = x1.floor().clamp(0.0, sw as f32 - 1.0) as u32;
    let y1 = y1.floor().clamp(0.0, sh as f32 - 1.0) as u32;
    let x2 = x2.ceil().clamp(x1 as f32 + 1.0, sw as f32) as u32;
    let y2 = y2.ceil().clamp(y1 as f32 + 1.0, sh as f32) as u32;
    let w = x2 - x1;
    let h = y2 - y1;
    let cropped = src.crop_imm(x1, y1, w, h);
    cropped.resize_exact(dst, dst, FilterType::Triangle).to_rgb8()
}

/// Crop + normalize directly to NCHW for ArcFace / DINOv2.
pub fn crop_to_nchw(
    src: &DynamicImage,
    bbox: (f32, f32, f32, f32),
    dst: u32,
    norm: Norm,
) -> Array4<f32> {
    let canvas = crop_resize(src, bbox, dst);
    to_nchw(&canvas, norm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageBuffer, Rgb};

    fn synth(w: u32, h: u32) -> DynamicImage {
        let buf: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(w, h, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        });
        DynamicImage::ImageRgb8(buf)
    }

    #[test]
    fn letterbox_square_center_pads_zero() {
        let img = synth(640, 640);
        let (canvas, lb) = letterbox(&img, 640);
        assert_eq!(canvas.dimensions(), (640, 640));
        assert_eq!(lb.pad_x, 0);
        assert_eq!(lb.pad_y, 0);
        assert!((lb.scale - 1.0).abs() < 1e-3);
    }

    #[test]
    fn letterbox_landscape_pads_top_bottom() {
        let img = synth(800, 400);
        let (canvas, lb) = letterbox(&img, 640);
        assert_eq!(canvas.dimensions(), (640, 640));
        assert_eq!(lb.pad_x, 0);
        assert!(lb.pad_y > 0);
        assert!((lb.scale - 0.8).abs() < 1e-3);
    }

    #[test]
    fn nchw_shape_norm() {
        let img = synth(112, 112);
        let buf = img.to_rgb8();
        let arr = to_nchw(&buf, Norm::ArcFace);
        assert_eq!(arr.shape(), &[1, 3, 112, 112]);
        // ArcFace: pixel 0 -> -1.0, pixel 255 -> ~1.0
        let v0 = arr[[0, 0, 0, 0]];
        assert!(v0 >= -1.001 && v0 <= 1.001, "v0={v0}");
    }

    #[test]
    fn unproject_inverts_letterbox() {
        let img = synth(800, 400);
        let (_canvas, lb) = letterbox(&img, 640);
        // A box that sits in the middle of the original 800x400 image
        // should round-trip with sub-pixel error.
        let (ox1, oy1, ox2, oy2) = (100.0, 50.0, 300.0, 200.0);
        let (mx1, my1, mx2, my2) = (
            ox1 * lb.scale + lb.pad_x as f32,
            oy1 * lb.scale + lb.pad_y as f32,
            ox2 * lb.scale + lb.pad_x as f32,
            oy2 * lb.scale + lb.pad_y as f32,
        );
        let (rx1, ry1, rx2, ry2) = lb.unproject(mx1, my1, mx2, my2, 800, 400);
        assert!((rx1 - ox1).abs() < 0.5);
        assert!((ry1 - oy1).abs() < 0.5);
        assert!((rx2 - ox2).abs() < 0.5);
        assert!((ry2 - oy2).abs() < 0.5);
    }
}
