//! Folder scanning and image decoding.

use std::path::{Path, PathBuf};
use std::time::Duration;

use image::AnimationDecoder;

pub const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "tif", "ico", "qoi", "pnm", "dds", "tga",
    "exr",
];

pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "m4v", "avi", "mpeg", "mpg", "ogv", "wmv", "flv", "3gp", "ts",
];

/// Longest edge an image is allowed to have after decoding; bigger media is
/// scaled down on load to keep the app light on the GPU.
const MAX_DIM: u32 = 4096;

/// Hard cap on the number of animated frames we keep in memory.
const MAX_ANIM_FRAMES: usize = 120;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MediaKind {
    Image,
    Video,
}

#[derive(Clone, Debug)]
pub struct MediaItem {
    pub path: PathBuf,
    pub kind: MediaKind,
    pub name: String,
}

pub fn ext_of(path: &Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_lowercase())
}

pub fn classify(path: &Path) -> Option<MediaKind> {
    let ext = ext_of(path)?;
    if IMAGE_EXTS.contains(&ext.as_str()) {
        Some(MediaKind::Image)
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        Some(MediaKind::Video)
    } else {
        None
    }
}

/// Scan a folder for media files, sorted by name.
pub fn scan_folder(dir: &Path) -> Vec<MediaItem> {
    let mut items = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            if let Some(kind) = classify(&p) {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                items.push(MediaItem {
                    path: p,
                    kind,
                    name,
                });
            }
        }
    }
    items
}

pub struct AnimatedFrame {
    pub delay: Duration,
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

pub struct LoadedImage {
    pub frames: Vec<AnimatedFrame>,
    pub width: u32,
    pub height: u32,
}

pub fn load_image(path: &Path) -> Result<LoadedImage, String> {
    let ext = ext_of(path).unwrap_or_default();

    // Animated GIFs.
    if ext == "gif" {
        let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        if let Ok(dec) = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(f)) {
            if let Ok(frames) = dec.into_frames().collect_frames() {
                if frames.len() > 1 {
                    return load_animated(frames);
                }
            }
        }
    }

    // Animated WebP.
    if ext == "webp" {
        let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        if let Ok(dec) = image::codecs::webp::WebPDecoder::new(std::io::BufReader::new(f)) {
            if dec.has_animation() {
                if let Ok(frames) = dec.into_frames().collect_frames() {
                    if frames.len() > 1 {
                        return load_animated(frames);
                    }
                }
            }
        }
    }

    // Still image.
    let img = image::open(path).map_err(|e| format!("cannot decode image: {e}"))?;
    let img = limit_dim(img);
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(LoadedImage {
        frames: vec![AnimatedFrame {
            delay: Duration::ZERO,
            rgba: rgba.into_raw(),
            width: w as usize,
            height: h as usize,
        }],
        width: w,
        height: h,
    })
}

fn load_animated(frames: Vec<image::Frame>) -> Result<LoadedImage, String> {
    let mut out = Vec::new();
    for frame in frames.into_iter().take(MAX_ANIM_FRAMES) {
        let (n, den) = frame.delay().numer_denom_ms();
        let millis = if den == 0 {
            0
        } else {
            (n as f64 / den as f64).round() as u64
        };
        let img = limit_dim(frame.into_buffer().into());
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        out.push(AnimatedFrame {
            delay: Duration::from_millis(millis.max(20)),
            rgba: rgba.into_raw(),
            width: w as usize,
            height: h as usize,
        });
    }
    if out.is_empty() {
        return Err("no frames".into());
    }
    let (width, height) = (out[0].width as u32, out[0].height as u32);
    Ok(LoadedImage {
        frames: out,
        width,
        height,
    })
}

fn limit_dim(img: image::DynamicImage) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    let max = w.max(h);
    if max <= MAX_DIM || max == 0 {
        img
    } else {
        let f = MAX_DIM as f32 / max as f32;
        let nw = ((w as f32 * f).round() as u32).max(1);
        let nh = ((h as f32 * f).round() as u32).max(1);
        img.resize(nw, nh, image::imageops::FilterType::Triangle)
    }
}
