//! FFmpeg-based video frame decoder running on a background thread.
//!
//! The decoder produces raw RGB24 frames paced to the video timeline; the UI
//! thread reads the latest frame each repaint. No audio is played yet.

use ffmpeg_next as ffmpeg;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ffmpeg::codec::context::Context as CodecContext;
use ffmpeg::format::Pixel;
use ffmpeg::media::Type;
use ffmpeg::software::scaling::flag::Flags;
use ffmpeg::software::scaling::Context as ScalingContext;
use ffmpeg::util::frame::video::Video;

/// AV_TIME_BASE — FFmpeg's internal timestamp resolution (microseconds).
const AV_TIME_BASE: f64 = 1_000_000.0;

#[derive(Clone, Default)]
pub struct Frame {
    pub rgb: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub pts: f64,
}

#[derive(Clone, Default)]
pub struct PlayerState {
    pub loaded: bool,
    pub playing: bool,
    pub position: f64,
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub error: Option<String>,
    /// True once a non-looping video has reached the end of the file.
    pub ended: bool,
}

enum Cmd {
    Play(bool),
    Seek(f64),
    SetLoop(bool),
    Stop,
}

pub struct VideoPlayer {
    tx: Sender<Cmd>,
    frame: Arc<Mutex<Frame>>,
    state: Arc<Mutex<PlayerState>>,
    handle: Option<JoinHandle<()>>,
}

impl VideoPlayer {
    /// `looping`: when `true` the video restarts at the end instead of
    /// stopping and setting [`PlayerState::ended`].
    pub fn open(path: PathBuf, looping: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        let frame = Arc::new(Mutex::new(Frame::default()));
        let state = Arc::new(Mutex::new(PlayerState {
            loaded: false,
            playing: false,
            position: 0.0,
            duration: 0.0,
            width: 0,
            height: 0,
            error: None,
            ended: false,
        }));
        let f2 = Arc::clone(&frame);
        let s2 = Arc::clone(&state);
        let handle = std::thread::Builder::new()
            .name("imora-video".into())
            .spawn(move || {
                if let Err(e) = decode_loop(&path, looping, &rx, &f2, &s2) {
                    if let Ok(mut s) = s2.lock() {
                        s.loaded = true;
                        s.error = Some(e);
                    }
                }
            })
            .ok();

        VideoPlayer {
            tx,
            frame,
            state,
            handle,
        }
    }

    pub fn play(&self) {
        let _ = self.tx.send(Cmd::Play(true));
    }

    pub fn pause(&self) {
        let _ = self.tx.send(Cmd::Play(false));
    }

    pub fn seek(&self, t: f64) {
        let _ = self.tx.send(Cmd::Seek(t));
    }

    pub fn set_loop(&self, looping: bool) {
        let _ = self.tx.send(Cmd::SetLoop(looping));
    }

    pub fn frame(&self) -> Frame {
        self.frame.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn state(&self) -> PlayerState {
        self.state.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn decode_loop(
    path: &Path,
    looping: bool,
    rx: &Receiver<Cmd>,
    out: &Arc<Mutex<Frame>>,
    st: &Arc<Mutex<PlayerState>>,
) -> Result<(), String> {
    let mut input = ffmpeg::format::input(path).map_err(|e| format!("cannot open: {e}"))?;
    let stream = input.streams().best(Type::Video).ok_or("no video stream")?;
    let stream_idx = stream.index();
    let timebase = stream.time_base();
    let context = CodecContext::from_parameters(stream.parameters()).map_err(|e| e.to_string())?;
    let mut decoder = context.decoder().video().map_err(|e| e.to_string())?;
    let (vw, vh) = (decoder.width(), decoder.height());
    let duration = input.duration() as f64 / AV_TIME_BASE;

    {
        let mut s = st.lock().unwrap();
        s.loaded = true;
        s.playing = true;
        s.duration = duration.max(0.0);
        s.width = vw;
        s.height = vh;
        s.error = None;
    }

    let mut scaler = ScalingContext::get(
        decoder.format(),
        vw,
        vh,
        Pixel::RGB24,
        vw,
        vh,
        Flags::BILINEAR,
    )
    .map_err(|e| format!("cannot create scaler: {e}"))?;

    let mut playing = true;
    let mut looping = looping;
    let mut pending_seek: Option<f64> = None;
    let mut start_pts = 0.0_f64;
    let mut start_time = Instant::now();
    let mut fv = Video::empty();
    let mut rgb = Video::empty();

    loop {
        // Drain command queue.
        loop {
            match rx.try_recv() {
                Ok(Cmd::Stop) => return Ok(()),
                Ok(Cmd::SetLoop(b)) => looping = b,
                Ok(Cmd::Play(p)) => {
                    if playing != p {
                        playing = p;
                        if let Ok(mut s) = st.lock() {
                            s.playing = p;
                        }
                        if p {
                            // Resuming: if we had already reached the end,
                            // replay from the start.
                            if st.lock().map(|s| s.ended).unwrap_or(false) {
                                let _ = input.seek(0, ..);
                                decoder.flush();
                                start_pts = 0.0;
                                if let Ok(mut s) = st.lock() {
                                    s.ended = false;
                                }
                            }
                            // Restart the pacing clock so we don't fast-forward
                            // to catch up with the paused time.
                            start_time = Instant::now();
                        } else {
                            // Pausing: remember where we stopped so a later
                            // resume continues smoothly from this position.
                            start_pts = st.lock().map(|s| s.position).unwrap_or(start_pts);
                            start_time = Instant::now();
                        }
                    }
                }
                Ok(Cmd::Seek(t)) => pending_seek = Some(t.max(0.0)),
                Err(_) => break,
            }
        }

        if let Some(t) = pending_seek.take() {
            let _ = input.seek((t * AV_TIME_BASE) as i64, ..);
            decoder.flush();
            start_pts = t;
            start_time = Instant::now();
            if let Ok(mut s) = st.lock() {
                s.ended = false;
            }
        }

        if !playing {
            match rx.recv_timeout(Duration::from_millis(16)) {
                Ok(Cmd::Stop) => return Ok(()),
                Ok(Cmd::SetLoop(b)) => looping = b,
                Ok(Cmd::Play(p)) => {
                    playing = p;
                    if let Ok(mut s) = st.lock() {
                        s.playing = p;
                    }
                    if p {
                        if st.lock().map(|s| s.ended).unwrap_or(false) {
                            let _ = input.seek(0, ..);
                            decoder.flush();
                            start_pts = 0.0;
                            if let Ok(mut s) = st.lock() {
                                s.ended = false;
                            }
                        }
                        start_time = Instant::now();
                    }
                }
                Ok(Cmd::Seek(t)) => {
                    // Render the frame at `t` even while paused.
                    let t = t.max(0.0);
                    let _ = input.seek((t * AV_TIME_BASE) as i64, ..);
                    decoder.flush();
                    start_pts = t;
                    start_time = Instant::now();
                    if let Ok(mut s) = st.lock() {
                        s.ended = false;
                    }
                    if let Some(pts) = decode_until_frame(
                        &mut input,
                        stream_idx,
                        &mut decoder,
                        &mut scaler,
                        &mut fv,
                        &mut rgb,
                        t,
                        timebase,
                    ) {
                        *out.lock().unwrap() = frame_from(&rgb, pts);
                        if let Ok(mut s) = st.lock() {
                            s.position = pts;
                        }
                    }
                }
                Err(_) => {}
            }
            continue;
        }

        if let Ok(mut s) = st.lock() {
            s.playing = true;
            s.error = None;
        }

        let next = input.packets().next();
        match next {
            Some((stream, packet)) if stream.index() == stream_idx => {
                if decoder.send_packet(&packet).is_err() {
                    continue;
                }
                while decoder.receive_frame(&mut fv).is_ok() {
                    let Some(pts) = fv.pts() else {
                        continue;
                    };
                    let secs =
                        pts as f64 * timebase.numerator() as f64 / timebase.denominator() as f64;
                    if secs < start_pts - 0.01 {
                        continue;
                    }
                    let target = secs - start_pts;
                    let elapsed = start_time.elapsed().as_secs_f64();
                    if target > elapsed {
                        std::thread::sleep(Duration::from_secs_f64(target - elapsed));
                    }
                    if scaler.run(&fv, &mut rgb).is_ok() {
                        *out.lock().unwrap() = frame_from(&rgb, secs);
                        if let Ok(mut s) = st.lock() {
                            s.position = secs;
                        }
                    }
                }
            }
            Some(_) => {}
            None => {
                // End of file.
                if looping {
                    // Loop back to the start.
                    let _ = input.seek(0, ..);
                    decoder.flush();
                    start_pts = 0.0;
                    start_time = Instant::now();
                } else {
                    // Stop and report the end so e.g. the slideshow can move on.
                    if let Ok(mut s) = st.lock() {
                        s.ended = true;
                        s.playing = false;
                    }
                    playing = false;
                }
            }
        }
    }
}

/// Decode packets until the first frame at or after `target` seconds,
/// scaling it into `rgb`. Returns the timestamp of that frame.
#[allow(clippy::too_many_arguments)]
fn decode_until_frame(
    input: &mut ffmpeg::format::context::Input,
    stream_idx: usize,
    decoder: &mut ffmpeg::codec::decoder::Video,
    scaler: &mut ScalingContext,
    fv: &mut Video,
    rgb: &mut Video,
    target: f64,
    tb: ffmpeg::Rational,
) -> Option<f64> {
    let mut guard = 0usize;
    loop {
        guard += 1;
        if guard > 4000 {
            return None;
        }
        let next = input.packets().next();
        match next {
            Some((stream, packet)) if stream.index() == stream_idx => {
                if decoder.send_packet(&packet).is_err() {
                    return None;
                }
                while decoder.receive_frame(fv).is_ok() {
                    let Some(pts) = fv.pts() else {
                        continue;
                    };
                    let secs = pts as f64 * tb.numerator() as f64 / tb.denominator() as f64;
                    if secs < target - 0.01 {
                        continue;
                    }
                    if scaler.run(fv, rgb).is_ok() {
                        return Some(secs);
                    }
                }
            }
            Some(_) => {}
            None => {
                let _ = input.seek(0, ..);
                decoder.flush();
            }
        }
    }
}

/// Grab a single frame from a video, scaled to fit `max_dim`, for thumbnails.
pub fn grab_frame(path: &Path, max_dim: u32) -> Result<(Vec<u8>, u32, u32), String> {
    let mut input = ffmpeg::format::input(path).map_err(|e| format!("{e}"))?;
    let stream = input.streams().best(Type::Video).ok_or("no video stream")?;
    let idx = stream.index();
    let context = CodecContext::from_parameters(stream.parameters()).map_err(|e| e.to_string())?;
    let mut decoder = context.decoder().video().map_err(|e| e.to_string())?;
    let (w, h) = (decoder.width(), decoder.height());
    let (tw, th) = fit_dim(w, h, max_dim);
    let mut scaler = ScalingContext::get(
        decoder.format(),
        w,
        h,
        Pixel::RGB24,
        tw,
        th,
        Flags::BILINEAR,
    )
    .map_err(|e| format!("{e}"))?;
    let mut fv = Video::empty();
    let mut rgb = Video::empty();
    let mut guard = 0usize;

    for (stream, packet) in input.packets() {
        if stream.index() != idx {
            continue;
        }
        if decoder.send_packet(&packet).is_err() {
            break;
        }
        while decoder.receive_frame(&mut fv).is_ok() {
            guard += 1;
            if guard > 300 {
                break;
            }
            if scaler.run(&fv, &mut rgb).is_ok() {
                let data = rgb.data(0);
                let stride = rgb.stride(0);
                let row = tw as usize * 3;
                let mut out = Vec::with_capacity(row * th as usize);
                for y in 0..th as usize {
                    let s = y * stride;
                    let e = (s + row).min(data.len());
                    out.extend_from_slice(&data[s..e]);
                }
                return Ok((out, tw, th));
            }
        }
    }
    Err("no frame".into())
}

fn frame_from(v: &Video, pts: f64) -> Frame {
    let w = v.width() as usize;
    let h = v.height() as usize;
    let stride = v.stride(0);
    let data = v.data(0);
    let row = w * 3;
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let s = (y * stride).min(data.len());
        let e = (s + row).min(data.len());
        rgb.extend_from_slice(&data[s..e]);
    }
    Frame {
        rgb,
        width: w as u32,
        height: h as u32,
        pts,
    }
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
