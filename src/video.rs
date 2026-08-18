//! FFmpeg-based video/audio decoder running on a background thread.
//!
//! The decoder produces raw RGB24 frames paced to the video timeline and, when
//! the file and an output device are available, feeds resampled audio to the
//! sound card; the UI thread reads the latest frame each repaint.

use ffmpeg_next as ffmpeg;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::audio::AudioOutput;
use ffmpeg::codec::context::Context as CodecContext;
use ffmpeg::codec::decoder::Audio as AudioDecoder;
use ffmpeg::format::sample::Type as SampleType;
use ffmpeg::format::Pixel;
use ffmpeg::media::Type;
use ffmpeg::software::resampling::Context as Resampler;
use ffmpeg::software::scaling::flag::Flags;
use ffmpeg::software::scaling::Context as ScalingContext;
use ffmpeg::util::channel_layout::ChannelLayout;
use ffmpeg::util::frame::audio::Audio as AudioFrame;
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
    /// Available video/audio tracks (stream index + label) and the current
    /// selection.
    pub video_tracks: Vec<TrackInfo>,
    pub audio_tracks: Vec<TrackInfo>,
    pub video_track: usize,
    pub audio_track: Option<usize>,
    /// Subtitle tracks, selection, and the currently visible subtitle text.
    pub subtitle_tracks: Vec<TrackInfo>,
    pub subtitle_track: Option<usize>,
    pub subtitle: Option<String>,
}

#[derive(Clone)]
pub struct TrackInfo {
    pub stream_index: usize,
    pub label: String,
}

enum Cmd {
    Play(bool),
    Seek(f64),
    SetLoop(bool),
    SetTracks {
        video_track: Option<usize>,
        audio_track: Option<usize>,
    },
    SetSubtitle(Option<usize>),
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
    /// stopping and setting [`PlayerState::ended`]. `audio_track` /
    /// `subtitle_track` select the initial streams (by stream index; `None`
    /// for audio uses the default track, `None` for subtitles disables them).
    pub fn open(
        path: PathBuf,
        looping: bool,
        audio_track: Option<usize>,
        subtitle_track: Option<usize>,
    ) -> Self {
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
            video_tracks: Vec::new(),
            audio_tracks: Vec::new(),
            video_track: 0,
            audio_track: None,
            subtitle_tracks: Vec::new(),
            subtitle_track: None,
            subtitle: None,
        }));
        let f2 = Arc::clone(&frame);
        let s2 = Arc::clone(&state);
        let handle = std::thread::Builder::new()
            .name("imora-video".into())
            .spawn(move || {
                if let Err(e) =
                    decode_loop(&path, looping, audio_track, subtitle_track, &rx, &f2, &s2)
                {
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

    /// Switch the active video/audio stream (by stream index; `None` for audio
    /// disables sound).
    pub fn set_tracks(&self, video_track: Option<usize>, audio_track: Option<usize>) {
        let _ = self.tx.send(Cmd::SetTracks {
            video_track,
            audio_track,
        });
    }

    /// Select a subtitle stream (`None` hides subtitles).
    pub fn set_subtitle(&self, subtitle_track: Option<usize>) {
        let _ = self.tx.send(Cmd::SetSubtitle(subtitle_track));
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
        // Don't join: the decode thread may be busy opening a large file or
        // the audio device, and joining on the UI thread would freeze arrow-key
        // navigation while a video prepares. Stop is queued and the thread
        // exits as soon as it can.
        drop(self.handle.take());
    }
}

/// Decodes and plays the audio stream of a video.
struct AudioPlayer {
    index: usize,
    decoder: AudioDecoder,
    resampler: Resampler,
    frame: AudioFrame,
    output: AudioOutput,
    pushed: u64,
    out_format: ffmpeg::format::Sample,
    out_layout: ChannelLayout,
}

impl AudioPlayer {
    /// Decode and play the audio stream with the given index (`None` → no audio).
    fn try_new(
        input: &mut ffmpeg::format::context::Input,
        stream_index: Option<usize>,
    ) -> Option<Self> {
        let stream_index = stream_index?;
        let stream = input.streams().find(|s| s.index() == stream_index)?;
        let index = stream.index();
        let context = CodecContext::from_parameters(stream.parameters()).ok()?;
        let decoder = context.decoder().audio().ok()?;
        let output = AudioOutput::try_new()?;

        let src_format = decoder.format();
        let src_layout = decoder.channel_layout();
        let src_rate = decoder.rate();
        let dst_format = ffmpeg::format::Sample::F32(SampleType::Packed);
        let dst_layout = ChannelLayout::default(output.channels as i32);
        let dst_rate = output.sample_rate;
        let resampler = Resampler::get(
            src_format, src_layout, src_rate, dst_format, dst_layout, dst_rate,
        )
        .ok()?;

        log::info!(
            "audio: {} Hz, {} ch -> device {} Hz, {} ch",
            src_rate,
            decoder.channels(),
            dst_rate,
            output.channels,
        );
        Some(Self {
            index,
            decoder,
            resampler,
            frame: AudioFrame::empty(),
            output,
            pushed: 0,
            out_format: dst_format,
            out_layout: dst_layout,
        })
    }

    fn send_packet(&mut self, packet: &ffmpeg::packet::Packet) {
        if self.decoder.send_packet(packet).is_err() {
            return;
        }
        while self.decoder.receive_frame(&mut self.frame).is_ok() {
            // Size the output from the rate ratio before resampling. `run`
            // allocates an empty output frame from the *input* sample count,
            // and `swr_convert_frame` then writes at most that many samples,
            // keeping the rest in its internal delay — which is dropped when
            // the resampler goes away. That made non-48 kHz tracks play back
            // too fast (e.g. 22050 Hz audio at ~2.2x) and lose the tail.
            let est = ((self.frame.samples() as u64 * self.output.sample_rate as u64)
                / self.frame.rate().max(1) as u64)
                .max(1) as usize;
            let mut converted = AudioFrame::new(self.out_format, est, self.out_layout);
            if self.resampler.run(&self.frame, &mut converted).is_ok() {
                // `plane::<f32>(0)` only exposes `nb_samples` (per-channel)
                // values, but a packed frame holds `nb_samples × channels`
                // interleaved samples — reading just the plane would drop half
                // of every stereo frame. Read the whole buffer instead.
                let count = converted.samples() * converted.channels() as usize;
                let buf = converted.data(0);
                let floats =
                    unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const f32, count) };
                self.output.push(floats);
                self.pushed += count as u64;
                // Keep the decode thread from out-running the hardware: the
                // ring only rides out jitter, so stall instead of dropping and
                // compressing the audio when the device is behind.
                self.output.wait_room(self.pushed);
            }
        }
    }

    /// Flush the decoder (after a seek/loop) and drop buffered audio.
    fn flush(&mut self) {
        self.decoder.flush();
        self.output.clear();
    }

    /// Drop buffered audio (pause / end) without touching the decoder.
    fn clear(&self) {
        self.output.clear();
    }
}

/// Decodes text subtitles from the selected subtitle stream.
struct SubtitlePlayer {
    index: usize,
    decoder: ffmpeg::codec::decoder::Subtitle,
    frame: ffmpeg::Subtitle,
    timebase: ffmpeg::Rational,
}

impl SubtitlePlayer {
    fn try_new(input: &mut ffmpeg::format::context::Input, stream_index: usize) -> Option<Self> {
        let stream = input.streams().find(|s| s.index() == stream_index)?;
        let context = CodecContext::from_parameters(stream.parameters()).ok()?;
        let decoder = context.decoder().subtitle().ok()?;
        Some(Self {
            index: stream_index,
            decoder,
            frame: ffmpeg::Subtitle::default(),
            timebase: stream.time_base(),
        })
    }

    /// Decode one packet; returns the event's start time (seconds) and text.
    fn decode(&mut self, packet: &ffmpeg::packet::Packet) -> Option<(f64, f64, String)> {
        let start = packet.pts().map(|pts| {
            pts as f64 * self.timebase.numerator() as f64 / self.timebase.denominator() as f64
        })?;
        let end = start
            + packet.duration() as f64 * self.timebase.numerator() as f64
                / self.timebase.denominator() as f64;
        if !self
            .decoder
            .decode(packet, &mut self.frame)
            .unwrap_or(false)
        {
            return None;
        }
        let mut lines = Vec::new();
        for rect in self.frame.rects() {
            let text = match rect {
                ffmpeg::subtitle::Rect::Text(t) => t.get().to_string(),
                ffmpeg::subtitle::Rect::Ass(a) => strip_ass(a.get()),
                _ => continue,
            };
            if !text.trim().is_empty() {
                lines.push(text);
            }
        }
        if lines.is_empty() {
            None
        } else {
            Some((start, end, lines.join("\n")))
        }
    }
}

/// MKV subtitles often decode as ASS even when the file stores SRT: drop the
/// `Layer,Style,Name,MarginL,MarginR,MarginV,Effect,` prefix and turn the
/// `\N` newline markers into real newlines.
fn strip_ass(line: &str) -> String {
    let text = line.rsplit_once(",,").map(|(_, t)| t).unwrap_or(line);
    text.replace("\\N", "\n").trim().to_string()
}

/// Rebuild the active video/audio decoders for the given streams and resync
/// playback to the current position.
#[allow(clippy::too_many_arguments)]
fn switch_tracks(
    input: &mut ffmpeg::format::context::Input,
    video_track: Option<usize>,
    audio_track: Option<usize>,
    stream_idx: &mut usize,
    timebase: &mut ffmpeg::Rational,
    decoder: &mut ffmpeg::codec::decoder::Video,
    scaler: &mut ScalingContext,
    vw: &mut u32,
    vh: &mut u32,
    audio: &mut Option<AudioPlayer>,
    audio_stream_idx: &mut Option<usize>,
    st: &Arc<Mutex<PlayerState>>,
    start_pts: &mut f64,
    start_time: &mut Instant,
    start_consumed: &mut u64,
) {
    let mut switched = false;

    // Video stream.
    if let Some(new_video) = video_track {
        if new_video != *stream_idx {
            if let Some(s) = input.streams().find(|s| s.index() == new_video) {
                if let Ok(context) = CodecContext::from_parameters(s.parameters()) {
                    if let Ok(new_decoder) = context.decoder().video() {
                        let nw = new_decoder.width();
                        let nh = new_decoder.height();
                        if let Ok(new_scaler) = ScalingContext::get(
                            new_decoder.format(),
                            nw,
                            nh,
                            Pixel::RGB24,
                            nw,
                            nh,
                            Flags::BILINEAR,
                        ) {
                            *decoder = new_decoder;
                            *scaler = new_scaler;
                            *stream_idx = new_video;
                            *timebase = s.time_base();
                            *vw = nw;
                            *vh = nh;
                            if let Ok(mut s2) = st.lock() {
                                s2.width = nw;
                                s2.height = nh;
                            }
                            switched = true;
                        }
                    }
                }
            }
        }
    }

    // Audio stream (`None` disables sound).
    if audio_track != *audio_stream_idx {
        *audio = AudioPlayer::try_new(input, audio_track);
        *audio_stream_idx = audio_track;
        switched = true;
    }

    if switched {
        let t = st.lock().map(|s| s.position).unwrap_or(0.0);
        let _ = input.seek((t * AV_TIME_BASE) as i64, ..);
        decoder.flush();
        *start_pts = t;
        *start_time = Instant::now();
        *start_consumed = audio.as_ref().map(|a| a.output.consumed()).unwrap_or(0);
        if let Some(a) = audio.as_mut() {
            a.flush();
        }
        if let Ok(mut s) = st.lock() {
            s.ended = false;
            s.video_track = video_track.unwrap_or(*stream_idx);
            s.audio_track = *audio_stream_idx;
        }
    }
}

/// How far ahead of the audio hardware clock the video may run.
const AV_LEAD_SECS: f64 = 0.05;

fn decode_loop(
    path: &Path,
    looping: bool,
    default_audio: Option<usize>,
    default_subtitle: Option<usize>,
    rx: &Receiver<Cmd>,
    out: &Arc<Mutex<Frame>>,
    st: &Arc<Mutex<PlayerState>>,
) -> Result<(), String> {
    let mut input = ffmpeg::format::input(path).map_err(|e| format!("cannot open: {e}"))?;

    // Enumerate the tracks so the UI can offer switching between them.
    let mut video_tracks: Vec<TrackInfo> = Vec::new();
    let mut audio_tracks: Vec<TrackInfo> = Vec::new();
    let mut subtitle_tracks: Vec<TrackInfo> = Vec::new();
    for s in input.streams() {
        match s.parameters().medium() {
            Type::Video => {
                // Skip still images (e.g. MKV cover art) — they can't be played.
                if is_playable_video(&s) {
                    video_tracks.push(TrackInfo {
                        stream_index: s.index(),
                        label: stream_label(&s),
                    });
                }
            }
            Type::Audio => audio_tracks.push(TrackInfo {
                stream_index: s.index(),
                label: stream_label(&s),
            }),
            Type::Subtitle => subtitle_tracks.push(TrackInfo {
                stream_index: s.index(),
                label: stream_label(&s),
            }),
            _ => {}
        }
    }

    let default_video = input.streams().best(Type::Video);
    let mut stream_idx = default_video
        .as_ref()
        .filter(|s| is_playable_video(s))
        .map(|s| s.index())
        .or_else(|| video_tracks.first().map(|t| t.stream_index))
        .unwrap_or(0);
    let stream = default_video
        .filter(|s| is_playable_video(s))
        .or_else(|| input.streams().find(|s| s.index() == stream_idx))
        .ok_or("no video stream")?;
    let mut timebase = stream.time_base();
    let context = CodecContext::from_parameters(stream.parameters()).map_err(|e| e.to_string())?;
    let mut decoder = context.decoder().video().map_err(|e| e.to_string())?;
    let (mut vw, mut vh) = (decoder.width(), decoder.height());
    let duration = input.duration() as f64 / AV_TIME_BASE;
    let mut audio_stream_idx = default_audio.or_else(|| {
        input
            .streams()
            .best(Type::Audio)
            .map(|s| s.index())
            .or_else(|| audio_tracks.first().map(|t| t.stream_index))
    });

    {
        let mut s = st.lock().unwrap();
        s.loaded = true;
        s.playing = true;
        s.duration = duration.max(0.0);
        s.width = vw;
        s.height = vh;
        s.error = None;
        s.video_tracks = video_tracks;
        s.audio_tracks = audio_tracks;
        s.video_track = stream_idx;
        s.audio_track = audio_stream_idx;
        s.subtitle_tracks = subtitle_tracks;
        s.subtitle_track = default_subtitle;
        s.subtitle = None;
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

    // Optional audio: only active if the file has an audio track and we can
    // open an output device. Otherwise playback stays silent.
    let mut audio = AudioPlayer::try_new(&mut input, audio_stream_idx);

    // Optional subtitles: decode text events when a track is selected.
    let mut subtitle = default_subtitle.and_then(|idx| SubtitlePlayer::try_new(&mut input, idx));
    let mut subtitle_stream_idx = default_subtitle;
    let mut pending_subs: VecDeque<(f64, f64, String)> = VecDeque::new();
    let mut current_sub_end: Option<f64> = None;

    let mut playing = true;
    let mut looping = looping;
    let mut pending_seek: Option<f64> = None;
    let mut start_pts = 0.0_f64;
    let mut start_time = Instant::now();
    // Anchor the pacing clock to the moment the audio device starts
    // consuming, not to when decode_loop() was entered — the device opens
    // earlier in AudioPlayer::try_new and may already be several ms ahead.
    let mut start_consumed = audio.as_ref().map(|a| a.output.consumed()).unwrap_or(0);
    let mut fv = Video::empty();
    let mut rgb = Video::empty();
    let mut last_stats = Instant::now();

    loop {
        // Drain command queue.
        loop {
            match rx.try_recv() {
                Ok(Cmd::Stop) => return Ok(()),
                Ok(Cmd::SetLoop(b)) => looping = b,
                Ok(Cmd::SetTracks {
                    video_track,
                    audio_track,
                }) => {
                    switch_tracks(
                        &mut input,
                        video_track,
                        audio_track,
                        &mut stream_idx,
                        &mut timebase,
                        &mut decoder,
                        &mut scaler,
                        &mut vw,
                        &mut vh,
                        &mut audio,
                        &mut audio_stream_idx,
                        st,
                        &mut start_pts,
                        &mut start_time,
                        &mut start_consumed,
                    );
                    clear_subs(st, &mut pending_subs, &mut current_sub_end);
                }
                Ok(Cmd::SetSubtitle(idx)) => {
                    if idx != subtitle_stream_idx {
                        subtitle = idx.and_then(|i| SubtitlePlayer::try_new(&mut input, i));
                        subtitle_stream_idx = idx;
                        clear_subs(st, &mut pending_subs, &mut current_sub_end);
                        if let Ok(mut s) = st.lock() {
                            s.subtitle_track = idx;
                        }
                    }
                }
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
                                if let Some(a) = audio.as_mut() {
                                    a.flush();
                                }
                                clear_subs(st, &mut pending_subs, &mut current_sub_end);
                            }
                            // Restart the pacing clock so we don't fast-forward
                            // to catch up with the paused time.
                            start_time = Instant::now();
                            start_consumed =
                                audio.as_ref().map(|a| a.output.consumed()).unwrap_or(0);
                        } else {
                            // Pausing: remember where we stopped so a later
                            // resume continues smoothly from this position.
                            start_pts = st.lock().map(|s| s.position).unwrap_or(start_pts);
                            start_time = Instant::now();
                            if let Some(a) = audio.as_ref() {
                                a.clear();
                            }
                            clear_subs(st, &mut pending_subs, &mut current_sub_end);
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
            start_consumed = audio.as_ref().map(|a| a.output.consumed()).unwrap_or(0);
            if let Ok(mut s) = st.lock() {
                s.ended = false;
            }
            if let Some(a) = audio.as_mut() {
                a.flush();
            }
            clear_subs(st, &mut pending_subs, &mut current_sub_end);
        }

        // Advance the subtitle display to the current position: show the
        // newest event that has started, clear once its end time passes.
        {
            let position = st.lock().map(|s| s.position).unwrap_or(0.0);
            let mut started: Option<(f64, String)> = None;
            while let Some((start, _, _)) = pending_subs.front() {
                if *start <= position {
                    let (_, end, text) = pending_subs.pop_front().unwrap();
                    started = Some((end, text));
                } else {
                    break;
                }
            }
            if let Some((end, text)) = started {
                current_sub_end = Some(end);
                if let Ok(mut s) = st.lock() {
                    s.subtitle = Some(text);
                }
            } else if let Some(end) = current_sub_end {
                if position > end {
                    current_sub_end = None;
                    if let Ok(mut s) = st.lock() {
                        s.subtitle = None;
                    }
                }
            }
        }

        if !playing {
            match rx.recv_timeout(Duration::from_millis(16)) {
                Ok(Cmd::Stop) => return Ok(()),
                Ok(Cmd::SetLoop(b)) => looping = b,
                Ok(Cmd::SetTracks {
                    video_track,
                    audio_track,
                }) => {
                    switch_tracks(
                        &mut input,
                        video_track,
                        audio_track,
                        &mut stream_idx,
                        &mut timebase,
                        &mut decoder,
                        &mut scaler,
                        &mut vw,
                        &mut vh,
                        &mut audio,
                        &mut audio_stream_idx,
                        st,
                        &mut start_pts,
                        &mut start_time,
                        &mut start_consumed,
                    );
                    clear_subs(st, &mut pending_subs, &mut current_sub_end);
                }
                Ok(Cmd::SetSubtitle(idx)) => {
                    if idx != subtitle_stream_idx {
                        subtitle = idx.and_then(|i| SubtitlePlayer::try_new(&mut input, i));
                        subtitle_stream_idx = idx;
                        clear_subs(st, &mut pending_subs, &mut current_sub_end);
                        if let Ok(mut s) = st.lock() {
                            s.subtitle_track = idx;
                        }
                    }
                }
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
                            if let Some(a) = audio.as_mut() {
                                a.flush();
                            }
                            clear_subs(st, &mut pending_subs, &mut current_sub_end);
                        }
                        start_time = Instant::now();
                        start_consumed = audio.as_ref().map(|a| a.output.consumed()).unwrap_or(0);
                    }
                }
                Ok(Cmd::Seek(t)) => {
                    // Render the frame at `t` even while paused.
                    let t = t.max(0.0);
                    let _ = input.seek((t * AV_TIME_BASE) as i64, ..);
                    decoder.flush();
                    start_pts = t;
                    start_time = Instant::now();
                    start_consumed = audio.as_ref().map(|a| a.output.consumed()).unwrap_or(0);
                    if let Ok(mut s) = st.lock() {
                        s.ended = false;
                    }
                    if let Some(a) = audio.as_mut() {
                        a.flush();
                    }
                    clear_subs(st, &mut pending_subs, &mut current_sub_end);
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
                    if let Some(a) = audio.as_ref() {
                        // Pace against the audio hardware clock: the device
                        // consumes samples at a steady rate, so this keeps
                        // audio and video in sync while never out-running the
                        // ring (which would drop and compress the audio).
                        let rate = a.output.sample_rate as f64 * a.output.channels as f64;
                        let consumed_now = a.output.consumed();
                        let clock = (consumed_now - start_consumed) as f64 / rate;
                        let ahead = target - clock;
                        if ahead > AV_LEAD_SECS {
                            std::thread::sleep(Duration::from_secs_f64(ahead - AV_LEAD_SECS));
                        }
                    } else {
                        let elapsed = start_time.elapsed().as_secs_f64();
                        if target > elapsed {
                            std::thread::sleep(Duration::from_secs_f64(target - elapsed));
                        }
                    }
                    if scaler.run(&fv, &mut rgb).is_ok() {
                        *out.lock().unwrap() = frame_from(&rgb, secs);
                        if let Ok(mut s) = st.lock() {
                            s.position = secs;
                        }
                    }
                }
            }
            Some((stream, packet)) if audio.as_ref().is_some_and(|a| a.index == stream.index()) => {
                if let Some(a) = audio.as_mut() {
                    a.send_packet(&packet);
                }
            }
            Some((stream, packet))
                if subtitle.as_ref().is_some_and(|s| s.index == stream.index()) =>
            {
                if let Some(sub) = subtitle.as_mut() {
                    if let Some((start, end, text)) = sub.decode(&packet) {
                        pending_subs.push_back((start, end, text));
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
                    start_consumed = audio.as_ref().map(|a| a.output.consumed()).unwrap_or(0);
                    if let Some(a) = audio.as_mut() {
                        a.flush();
                    }
                    clear_subs(st, &mut pending_subs, &mut current_sub_end);
                } else {
                    // Stop and report the end so e.g. the slideshow can move on.
                    if let Ok(mut s) = st.lock() {
                        s.ended = true;
                        s.playing = false;
                    }
                    playing = false;
                    if let Some(a) = audio.as_ref() {
                        a.clear();
                    }
                    clear_subs(st, &mut pending_subs, &mut current_sub_end);
                }
            }
        }

        if last_stats.elapsed().as_secs_f64() > 3.0 {
            last_stats = Instant::now();
            if let Some(a) = audio.as_ref() {
                let (drops, underruns) = a.output.stats();
                log::info!(
                    "audio stats: drops={drops} underruns={underruns} pos={:.1}s",
                    st.lock().map(|s| s.position).unwrap_or(0.0)
                );
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

/// A video stream is playable if it declares a frame rate — still images
/// (e.g. MKV cover art) report 0/0 and can't be played as video.
fn is_playable_video(stream: &ffmpeg::format::stream::Stream) -> bool {
    let rate = stream.avg_frame_rate();
    rate.numerator() != 0 || rate.denominator() != 0
}

/// Drop queued subtitles and hide any currently displayed one.
fn clear_subs(
    st: &Arc<Mutex<PlayerState>>,
    pending_subs: &mut VecDeque<(f64, f64, String)>,
    current_sub_end: &mut Option<f64>,
) {
    pending_subs.clear();
    *current_sub_end = None;
    if let Ok(mut s) = st.lock() {
        s.subtitle = None;
    }
}

/// Human-readable label for a stream: "language · codec", or a fallback.
fn stream_label(stream: &ffmpeg::format::stream::Stream) -> String {
    let codec = stream.parameters().id().name().to_string();
    let language = stream
        .metadata()
        .get("language")
        .unwrap_or_default()
        .to_string();
    match (language.is_empty(), codec.is_empty()) {
        (false, false) => format!("{language} · {codec}"),
        (false, true) => language,
        (true, false) => codec,
        (true, true) => format!("Stream {}", stream.index()),
    }
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
