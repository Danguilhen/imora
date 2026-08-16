//! Media metadata for the info panel (`I` key / ⓘ button).

use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::media::{self, MediaKind};
use ffmpeg_next as ffmpeg;

pub struct MediaInfo {
    pub title: String,
    pub entries: Vec<(String, String)>,
}

pub fn gather(path: &Path) -> MediaInfo {
    let title = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut entries = Vec::new();

    if let Ok(md) = std::fs::metadata(path) {
        entries.push(("Size".to_string(), human_bytes(md.len())));
        if let Ok(t) = md.modified() {
            if let Ok(d) = t.duration_since(UNIX_EPOCH) {
                entries.push(("Modified".to_string(), format_unix_time(d.as_secs())));
            }
        }
    }

    match media::classify(path) {
        Some(MediaKind::Image) => image_info(path, &mut entries),
        Some(MediaKind::Video) => video_info(path, &mut entries),
        None => {}
    }

    MediaInfo { title, entries }
}

fn image_info(path: &Path, entries: &mut Vec<(String, String)>) {
    if let Ok((w, h)) = image::image_dimensions(path) {
        entries.push(("Dimensions".to_string(), format!("{w} × {h}")));
    }
    if let Ok(format) = image::ImageFormat::from_path(path) {
        entries.push(("Format".to_string(), format!("{format:?}").to_uppercase()));
    }
}

fn video_info(path: &Path, entries: &mut Vec<(String, String)>) {
    let Ok(input) = ffmpeg::format::input(path) else {
        return;
    };

    let duration = input.duration() as f64 / 1_000_000.0;
    if duration > 0.0 {
        entries.push(("Duration".to_string(), fmt_duration(duration)));
    }
    let bit_rate = input.bit_rate();
    if bit_rate > 0 {
        entries.push(("Bitrate".to_string(), format_kbps(bit_rate)));
    }
    entries.push(("Container".to_string(), input.format().name().to_string()));

    for stream in input.streams() {
        let params = stream.parameters();
        let kind = params.medium();
        let Some(context) = ffmpeg::codec::context::Context::from_parameters(params).ok() else {
            continue;
        };
        let codec = context
            .codec()
            .map(|c| c.name().to_string())
            .unwrap_or_default();
        match kind {
            ffmpeg::media::Type::Video => match context.decoder().video() {
                Ok(decoder) => entries.push((
                    "Video".to_string(),
                    format!("{codec}, {} × {}", decoder.width(), decoder.height()),
                )),
                Err(_) => entries.push(("Video".to_string(), codec)),
            },
            ffmpeg::media::Type::Audio => match context.decoder().audio() {
                Ok(decoder) => entries.push((
                    "Audio".to_string(),
                    format!("{codec}, {} Hz, {} ch", decoder.rate(), decoder.channels()),
                )),
                Err(_) => entries.push(("Audio".to_string(), codec)),
            },
            _ => {}
        }
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut i = 0;
    while size >= 1024.0 && i < UNITS.len() - 1 {
        size /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[i])
    }
}

fn format_kbps(bits: i64) -> String {
    if bits <= 0 {
        return "—".to_string();
    }
    let kbps = bits as f64 / 1000.0;
    if kbps >= 1000.0 {
        format!("{:.1} Mb/s", kbps / 1000.0)
    } else {
        format!("{kbps:.0} kb/s")
    }
}

fn fmt_duration(secs: f64) -> String {
    let s = secs as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

fn format_unix_time(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let (year, month, day) = civil_from_days(days);
    let tod = secs % 86_400;
    let (hh, mm, ss) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    format!("{year:04}-{month:02}-{day:02} {hh:02}:{mm:02}:{ss:02} UTC")
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_metadata_has_size_and_dimensions() {
        let dir = std::env::temp_dir().join("imora-metadata-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pic.png");

        let img = image::RgbaImage::from_pixel(64, 32, image::Rgba([0, 128, 255, 255]));
        img.save(&path).unwrap();

        let info = gather(&path);
        assert_eq!(info.title, "pic.png");
        let get = |k: &str| {
            info.entries
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("Dimensions").as_deref(), Some("64 × 32"));
        assert_eq!(get("Format").as_deref(), Some("PNG"));
        assert!(get("Size").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
