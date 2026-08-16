//! Audio output for video playback (via cpal).
//!
//! The video decode thread resamples decoded audio to interleaved `f32` and
//! pushes it into a small ring buffer. The cpal callback — running on the audio
//! device's thread — pulls samples out, converts them to the device's sample
//! format, and feeds the hardware. When the ring buffer is full the decode
//! thread blocks, so the audio hardware paces playback while the existing
//! wall-clock logic paces the video.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;

/// How much audio to buffer before applying backpressure to the decode thread.
const BUFFER_SECS: f32 = 0.5;

pub struct AudioOutput {
    // Kept alive: dropping the stream stops playback.
    #[allow(dead_code)]
    stream: cpal::Stream,
    buffer: Arc<AudioBuffer>,
    pub sample_rate: u32,
    pub channels: u16,
}

struct AudioBuffer {
    buf: Mutex<VecDeque<f32>>,
    space: Condvar,
    capacity: usize,
}

impl AudioBuffer {
    fn new(seconds: f32, rate: u32) -> Self {
        Self {
            buf: Mutex::new(VecDeque::new()),
            space: Condvar::new(),
            capacity: ((rate as f32) * seconds.max(0.05)) as usize,
        }
    }

    /// Push interleaved f32 samples, blocking while the buffer is full so the
    /// decode thread is paced to the audio hardware. If the device appears to
    /// have wedged (no progress for a while) the rest is dropped instead of
    /// hanging the decode thread.
    fn push(&self, samples: &[f32]) {
        let mut buf = self.buf.lock().unwrap();
        let mut pushed = 0usize;
        let mut waited = Duration::ZERO;
        while pushed < samples.len() {
            while buf.len() >= self.capacity {
                if waited > Duration::from_secs(2) {
                    self.space.notify_all();
                    return;
                }
                let (guard, _) = self
                    .space
                    .wait_timeout(buf, Duration::from_millis(100))
                    .unwrap();
                buf = guard;
                waited += Duration::from_millis(100);
            }
            let room = self.capacity - buf.len();
            let take = room.min(samples.len() - pushed);
            buf.extend(samples[pushed..pushed + take].iter().copied());
            pushed += take;
        }
        self.space.notify_all();
    }

    fn clear(&self) {
        self.buf.lock().unwrap().clear();
        self.space.notify_all();
    }
}

impl AudioOutput {
    /// Open the default output device and start the stream. Returns `None`
    /// (playback stays silent) if there is no usable device or format.
    pub fn try_new() -> Option<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let supported = pick_config(&device)?;
        let format = supported.sample_format();
        let sample_rate = supported.sample_rate();
        let channels = supported.channels();
        let config: cpal::StreamConfig = supported.into();

        let buffer = Arc::new(AudioBuffer::new(BUFFER_SECS, sample_rate));
        let stream = match format {
            SampleFormat::F32 => build_stream::<f32>(&device, config, Arc::clone(&buffer)),
            SampleFormat::I16 => build_stream::<i16>(&device, config, Arc::clone(&buffer)),
            SampleFormat::I32 => build_stream::<i32>(&device, config, Arc::clone(&buffer)),
            _ => return None,
        }
        .ok()?;

        stream.play().ok()?;
        log::info!("audio output opened: {sample_rate} Hz, {channels} ch, {format:?}");
        Some(Self {
            stream,
            buffer,
            sample_rate,
            channels,
        })
    }

    /// Feed interleaved f32 samples (blocking when the buffer is full).
    pub fn push(&self, samples: &[f32]) {
        self.buffer.push(samples);
    }

    /// Drop all buffered audio (pause / seek / loop / end).
    pub fn clear(&self) {
        self.buffer.clear();
    }
}

/// Pick a device config we can feed easily: prefer the device default, else the
/// first supported config, restricted to formats we convert to directly.
fn pick_config(device: &cpal::Device) -> Option<cpal::SupportedStreamConfig> {
    if let Ok(config) = device.default_output_config() {
        if matches!(
            config.sample_format(),
            SampleFormat::F32 | SampleFormat::I16 | SampleFormat::I32
        ) {
            return Some(config);
        }
    }
    for range in device.supported_output_configs().ok()? {
        if matches!(
            range.sample_format(),
            SampleFormat::F32 | SampleFormat::I16 | SampleFormat::I32
        ) {
            return Some(range.with_max_sample_rate());
        }
    }
    None
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    buffer: Arc<AudioBuffer>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let mut buf = buffer.buf.lock().unwrap();
            for slot in data.iter_mut() {
                let sample = buf.pop_front().unwrap_or(0.0);
                *slot = T::from_sample(sample);
            }
            buffer.space.notify_all();
        },
        move |err| log::warn!("audio stream error: {err:?}"),
        None,
    )
}
