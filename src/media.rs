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

/// If `s` is a network stream URL (`scheme://rest`, like mpv accepts), return
/// it trimmed. ffmpeg opens these itself, so no download step is needed.
pub fn as_url(s: &str) -> Option<&str> {
    let t = s.trim();
    let (scheme, rest) = t.split_once("://")?;
    let mut chars = scheme.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    if rest.is_empty() {
        return None;
    }
    Some(t)
}

/// True when the URL points straight at a media file (extension check after
/// stripping query/fragment), so it can skip stream-site extraction.
pub fn looks_like_media_file(url: &str) -> bool {
    let clean = url.split(['?', '#']).next().unwrap_or(url);
    matches!(classify(Path::new(clean)), Some(MediaKind::Video))
}

/// Resolve a site URL (YouTube etc.) to a direct media URL with yt-dlp,
/// mirroring mpv's ytdl hook. The best *single-file* format is requested so
/// ffmpeg can play everything from one URL.
///
/// The web ("default") client's stream URLs increasingly 403 for external
/// players (token enforcement); android/mweb URLs play fine in ffmpeg, so
/// request those clients explicitly. Scoped to the youtube extractor, other
/// sites ignore this.
pub fn resolve_stream(url: &str) -> Result<String, String> {
    for exe in ["yt-dlp", "youtube-dl"] {
        let Ok(out) = std::process::Command::new(exe)
            .args([
                "-g",
                "--no-warnings",
                "--extractor-args",
                "youtube:player_client=android,mweb",
                "-f",
                "b",
                url,
            ])
            .output()
        else {
            continue; // extractor not installed; try the next one
        };
        if out.status.success() {
            if let Some(line) = String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                .find(|l| l.starts_with("http"))
            {
                return Ok(line.to_string());
            }
        }
        // Surface the extractor's own complaint (bot-check, geo-block, bad
        // URL, ...) instead of a generic failure; keep it toast-sized.
        let reason = String::from_utf8_lossy(&out.stderr)
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("no error output")
            .to_string();
        log::warn!("{exe} failed on {url}: {reason}");
        return Err(format!("{exe} could not extract a stream: {}", {
            let mut r = reason;
            if r.len() > 160 {
                r.truncate(r.char_indices().nth(157).map(|(i, _)| i).unwrap_or(160));
                r.push('…');
            }
            r
        }));
    }
    Err("install yt-dlp to play URLs from streaming sites".into())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_stream_urls() {
        assert_eq!(
            as_url(" https://example.com/live.m3u8 "),
            Some("https://example.com/live.m3u8")
        );
        assert_eq!(
            as_url("rtsp://cam.local/stream"),
            Some("rtsp://cam.local/stream")
        );
        assert_eq!(
            as_url("http://host/path?query=1&x=2"),
            Some("http://host/path?query=1&x=2")
        );
        // Local paths, bare words, and junk are not URLs.
        assert_eq!(as_url("/home/user/video.mp4"), None);
        assert_eq!(as_url("./relative/file.webm"), None);
        assert_eq!(as_url("notaurl"), None);
        assert_eq!(as_url("1https://bad.scheme"), None);
        assert_eq!(as_url("https://"), None);
        // Prose around the link must not produce a garbage URL.
        assert_eq!(as_url("watch this: https://x.co/a now"), None);
        // Schemes are case-insensitive (pasted addresses may be uppercase).
        assert_eq!(
            as_url("HTTPS://Example.com/watch?v=1"),
            Some("HTTPS://Example.com/watch?v=1")
        );
    }

    #[test]
    fn paste_finds_the_url_token_in_clipboard_text() {
        // Mirrors handle_paste's tokenizer: first whitespace-separated token
        // that is a URL wins, whatever else the clipboard carries.
        fn pick(text: &str) -> Option<&str> {
            text.split_whitespace().find_map(as_url)
        }
        assert_eq!(
            pick("  https://www.youtube.com/watch?v=F7fe9pa8OeE\n"),
            Some("https://www.youtube.com/watch?v=F7fe9pa8OeE")
        );
        assert_eq!(
            pick("Check this out:\nhttps://youtu.be/abc123 ?si=x"),
            Some("https://youtu.be/abc123")
        );
        assert_eq!(pick("no link here"), None);
    }

    #[test]
    fn http_gate_is_case_insensitive() {
        // The spawn_load gate lowercases before prefix-matching; keep this
        // invariant honest for pasted HTTPS:// URLs.
        let gate = |u: &str| u.len() >= 4 && u[..4].eq_ignore_ascii_case("http");
        assert!(gate("https://a.co/v"));
        assert!(gate("HTTP://a.co/v"));
        assert!(gate("Https://a.co/v"));
        assert!(!gate("ftp://a.co/v"));
    }

    #[test]
    fn media_files_are_recognized_through_query_strings() {
        assert!(looks_like_media_file("https://cdn.example.com/video.mp4"));
        assert!(looks_like_media_file(
            "https://example.com/v.webm?token=1&sig=x#frag"
        ));
        // Pages, playlists, and extension-less streams are not media files.
        assert!(!looks_like_media_file(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        ));
        assert!(!looks_like_media_file("https://example.com/live.m3u8"));
        assert!(!looks_like_media_file("https://example.com/"));
    }
}
