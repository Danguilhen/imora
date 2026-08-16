//! Audio output for video playback (via cpal).
//!
//! The video decode thread resamples decoded audio to interleaved `f32` and
//! pushes it into a small ring buffer. The cpal callback — running on the audio
//! device's thread — pulls samples out, converts them to the device's sample
//! format, and feeds the hardware. Pushing is non-blocking: the video clock
//! paces playback and the ring buffer only rides out jitter, so the decode
//! thread never stalls on audio (which would make the video stutter).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;

/// How much audio to buffer to ride out decode/demux jitter.
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
    capacity: usize,
}

impl AudioBuffer {
    fn new(seconds: f32, rate: u32) -> Self {
        Self {
            buf: Mutex::new(VecDeque::new()),
            capacity: ((rate as f32) * seconds.max(0.05)) as usize,
        }
    }

    /// Push interleaved f32 samples. Never blocks: if the buffer is full, the
    /// oldest samples are dropped to keep the audio as current as possible.
    /// Samples are clamped to `[-1, 1]` to guard against resampling overshoot
    /// clipping integer output devices.
    fn push(&self, samples: &[f32]) {
        let mut buf = self.buf.lock().unwrap();
        for &sample in samples {
            if buf.len() >= self.capacity {
                buf.pop_front();
            }
            buf.push_back(sample.clamp(-1.0, 1.0));
        }
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

    /// Feed interleaved f32 samples (never blocks).
    pub fn push(&self, samples: &[f32]) {
        self.buffer.push(samples);
    }

    /// Drop all buffered audio (pause / seek / loop / end).
    pub fn clear(&self) {
        self.buffer.buf.lock().unwrap().clear();
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
        },
        move |err| log::warn!("audio stream error: {err:?}"),
        None,
    )
}
