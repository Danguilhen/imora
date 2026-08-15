//! Filmstrip thumbnail generation (runs on a background thread).

use std::path::{Path, PathBuf};

use crate::media::MediaKind;

const MAX: u32 = 160;

pub struct ThumbJob {
    pub path: PathBuf,
    pub kind: MediaKind,
}

pub struct ThumbResult {
    pub path: PathBuf,
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

pub fn make_thumb(job: &ThumbJob) -> Option<ThumbResult> {
    match job.kind {
        MediaKind::Image => image_thumb(&job.path),
        MediaKind::Video => video_thumb(&job.path),
    }
}

fn image_thumb(path: &Path) -> Option<ThumbResult> {
    let img = image::open(path).ok()?;
    let (w, h) = (img.width(), img.height());
    let (tw, th) = fit_dim(w, h, MAX);
    let img = img.resize(tw, th, image::imageops::FilterType::Triangle);
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(ThumbResult {
        path: path.to_path_buf(),
        rgba: rgba.into_raw(),
        width: w as usize,
        height: h as usize,
    })
}

fn video_thumb(path: &Path) -> Option<ThumbResult> {
    let (rgb, w, h) = crate::video::grab_frame(path, MAX).ok()?;
    let mut rgba = Vec::with_capacity(rgb.len() + rgb.len() / 3);
    for px in rgb.chunks(3) {
        rgba.extend_from_slice(px);
        rgba.push(255);
    }
    Some(ThumbResult {
        path: path.to_path_buf(),
        rgba,
        width: w as usize,
        height: h as usize,
    })
}

fn fit_dim(w: u32, h: u32, max: u32) -> (u32, u32) {
    if w == 0 || h == 0 || max == 0 {
        return (1, 1);
    }
    let m = w.max(h);
    if m <= max {
        (w, h)
    } else {
        let f = max as f32 / m as f32;
        (
            ((w as f32 * f).round() as u32).max(1),
            ((h as f32 * f).round() as u32).max(1),
        )
    }
}
