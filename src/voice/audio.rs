//! Local Audio I/O Module
//!
//! Provides real-time audio capture and playback using:
//! - cpal: Cross-platform audio I/O (microphone input, speaker output)
//! - rodio: Audio playback and mixing
//! - hound: WAV file format handling
//!
//! # Architecture
//!
//! ```text
//! Microphone → AudioInput → AudioBuffer → (VAD) → STT
//!                                               ↓
//! Speaker ← AudioOutput ← AudioBuffer ← TTS
//! ```
//!
//! # Features
//! - Real-time audio capture from default microphone
//! - Audio playback to default speakers
//! - Ring buffer for continuous streaming
//! - Audio format conversion (f32/i16)
//! - WAV file recording and playback

use anyhow::{Result, Context, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::collections::VecDeque;
use tracing::{info, error, debug};

/// Default sample rate for audio processing
pub const DEFAULT_SAMPLE_RATE: u32 = 16000; // 16kHz - optimal for Whisper STT

/// Default number of channels (mono for speech)
pub const DEFAULT_CHANNELS: u16 = 1;

/// Buffer size for audio chunks (100ms at 16kHz)
pub const DEFAULT_BUFFER_SIZE: usize = 1600;

/// Audio format specification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// 16-bit signed integer
    I16,
    /// 32-bit floating point
    F32,
}

/// Audio stream configuration
#[derive(Debug, Clone)]
pub struct AudioConfig {
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Number of channels (1 = mono, 2 = stereo)
    pub channels: u16,
    /// Sample format
    pub sample_format: SampleFormat,
    /// Buffer size in samples
    pub buffer_size: usize,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: DEFAULT_CHANNELS,
            sample_format: SampleFormat::F32,
            buffer_size: DEFAULT_BUFFER_SIZE,
        }
    }
}

/// Audio input (microphone) handler
pub struct AudioInput {
    config: AudioConfig,
    device: cpal::Device,
    stream_config: cpal::StreamConfig,
    is_running: Arc<AtomicBool>,
    audio_buffer: Arc<Mutex<VecDeque<f32>>>,
}

/// Audio output (speaker) handler
pub struct AudioOutput {
    config: AudioConfig,
    device: cpal::Device,
    stream_config: cpal::StreamConfig,
    sink: rodio::Sink,
    _stream: rodio::OutputStream,
}

/// Audio buffer for streaming data
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    samples: Vec<f32>,
    sample_rate: u32,
    channels: u16,
}

impl AudioBuffer {
    /// Create a new audio buffer
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            samples: Vec::new(),
            sample_rate,
            channels,
        }
    }

    /// Create from raw samples
    pub fn from_samples(samples: Vec<f32>, sample_rate: u32, channels: u16) -> Self {
        Self {
            samples,
            sample_rate,
            channels,
        }
    }

    /// Push samples to the buffer
    pub fn push(&mut self, samples: &[f32]) {
        self.samples.extend_from_slice(samples);
    }

    /// Get all samples
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// Get samples as i16 (for WAV export)
    pub fn samples_i16(&self) -> Vec<i16> {
        self.samples.iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect()
    }

    /// Clear the buffer
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Get buffer duration in seconds
    pub fn duration_secs(&self) -> f64 {
        self.samples.len() as f64 / (self.sample_rate as f64 * self.channels as f64)
    }

    /// Get number of samples
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Resample to a different sample rate (simple linear interpolation)
    pub fn resample(&self, target_rate: u32) -> Self {
        if self.sample_rate == target_rate {
            return self.clone();
        }

        let ratio = target_rate as f64 / self.sample_rate as f64;
        let new_len = (self.samples.len() as f64 * ratio) as usize;
        let mut new_samples = Vec::with_capacity(new_len);

        for i in 0..new_len {
            let src_idx = i as f64 / ratio;
            let idx_floor = src_idx.floor() as usize;
            let idx_ceil = (idx_floor + 1).min(self.samples.len() - 1);
            let frac = src_idx - idx_floor as f64;

            let val = self.samples[idx_floor] * (1.0 - frac as f32)
                    + self.samples[idx_ceil] * frac as f32;
            new_samples.push(val);
        }

        Self {
            samples: new_samples,
            sample_rate: target_rate,
            channels: self.channels,
        }
    }
}

impl AudioInput {
    /// Create a new audio input handler with default device
    pub fn new(config: AudioConfig) -> Result<Self> {
        let host = cpal::default_host();
        let device = host.default_input_device()
            .context("No input device available (microphone not found)")?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());
        info!("Using input device: {}", device_name);

        let mut supported_configs = device.supported_input_configs()
            .context("Failed to get supported input configs")?;

        // Find a supported config that matches our requirements
        let supported_config = supported_configs.find(|c| {
            c.sample_format() == cpal::SampleFormat::F32 ||
            c.sample_format() == cpal::SampleFormat::I16
        }).context("No supported sample format found")?;

        let sample_format = supported_config.sample_format();
        let sample_rate = supported_config.min_sample_rate().0
            .max(config.sample_rate)
            .min(supported_config.max_sample_rate().0);

        let stream_config = cpal::StreamConfig {
            channels: config.channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Fixed(config.buffer_size as u32),
        };

        info!("Input config: {:?}Hz, {} channels, {:?}",
              sample_rate, config.channels, sample_format);

        Ok(Self {
            config,
            device,
            stream_config,
            is_running: Arc::new(AtomicBool::new(false)),
            audio_buffer: Arc::new(Mutex::new(VecDeque::new())),
        })
    }

    /// Start capturing audio from microphone
    pub fn start<F>(&self, mut callback: F) -> Result<cpal::Stream>
    where
        F: FnMut(&[f32]) + Send + 'static,
    {
        if self.is_running.load(Ordering::SeqCst) {
            bail!("Audio input already running");
        }

        let is_running = self.is_running.clone();
        let buffer = self.audio_buffer.clone();

        let err_fn = |err| error!("Audio input error: {}", err);

        let stream: cpal::Stream = match self.device.default_input_config()?.sample_format() {
            cpal::SampleFormat::F32 => self.device.build_input_stream(
                &self.stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if is_running.load(Ordering::SeqCst) {
                        if let Ok(mut buf) = buffer.lock() {
                            buf.extend(data.iter().copied());
                        }
                        callback(data);
                    }
                },
                err_fn,
                None,
            )?,
            cpal::SampleFormat::I16 => self.device.build_input_stream(
                &self.stream_config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if is_running.load(Ordering::SeqCst) {
                        let samples: Vec<f32> = data.iter()
                            .map(|&s| (s as f32 / i16::MAX as f32).clamp(-1.0, 1.0))
                            .collect();
                        if let Ok(mut buf) = buffer.lock() {
                            buf.extend(samples.iter().copied());
                        }
                        callback(&samples);
                    }
                },
                err_fn,
                None,
            )?,
            format => bail!("Unsupported sample format: {:?}", format),
        };

        stream.play()?;
        self.is_running.store(true, Ordering::SeqCst);
        info!("Audio input started");
        Ok(stream)
    }

    pub fn stop(&self, stream: &cpal::Stream) -> Result<()> {
        self.is_running.store(false, Ordering::SeqCst);
        stream.pause()?;
        info!("Audio input stopped");
        Ok(())
    }

    pub fn read_buffer(&self) -> Vec<f32> {
        if let Ok(mut buf) = self.audio_buffer.lock() {
            buf.drain(..).collect()
        } else {
            Vec::new()
        }
    }

    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    pub fn device_name(&self) -> Result<String> {
        self.device.name().map_err(|e| anyhow::anyhow!(e))
    }

    pub fn sample_rate(&self) -> u32 {
        self.stream_config.sample_rate.0
    }
}

impl AudioOutput {
    /// Create a new audio output handler with default device
    pub fn new(config: AudioConfig) -> Result<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device()
            .context("No output device available (speakers not found)")?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());
        info!("Using output device: {}", device_name);

        let supported_config = device.default_output_config()?;
        let sample_rate = supported_config.sample_rate().0;

        let stream_config = cpal::StreamConfig {
            channels: config.channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Fixed(config.buffer_size as u32),
        };

        info!("Output config: {:?}Hz, {} channels",
              sample_rate, config.channels);

        // Create rodio output stream
        let (_stream, stream_handle) = rodio::OutputStream::try_default()
            .context("Failed to create audio output stream")?;
        let sink = rodio::Sink::try_new(&stream_handle)
            .context("Failed to create audio sink")?;

        Ok(Self {
            config,
            device,
            stream_config,
            sink,
            _stream,
        })
    }

    /// Play audio samples
    pub fn play(&self, samples: &[f32]) -> Result<()> {
        if samples.is_empty() {
            return Ok(());
        }

        // Convert f32 samples to rodio's expected format
        let samples_i16: Vec<i16> = samples.iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect();

        // Create a source from the samples
        let source = rodio::buffer::SamplesBuffer::new(
            self.stream_config.channels,
            self.stream_config.sample_rate.0,
            samples_i16
        );

        self.sink.append(source);

        debug!("Queued {} samples for playback", samples.len());
        Ok(())
    }

    /// Play audio from an AudioBuffer
    pub fn play_buffer(&self, buffer: &AudioBuffer) -> Result<()> {
        // Resample if necessary
        if buffer.sample_rate != self.stream_config.sample_rate.0 {
            let resampled = buffer.resample(self.stream_config.sample_rate.0);
            self.play(&resampled.samples)?;
        } else {
            self.play(&buffer.samples)?;
        }
        Ok(())
    }

    /// Play a WAV file
    pub fn play_wav(&self, path: &std::path::Path) -> Result<()> {
        let file = std::fs::File::open(path)
            .context(format!("Failed to open WAV file: {:?}", path))?;
        let source = rodio::Decoder::new(std::io::BufReader::new(file))
            .context("Failed to decode WAV file")?;

        self.sink.append(source);
        info!("Playing WAV file: {:?}", path);
        Ok(())
    }

    /// Wait for all audio to finish playing
    pub fn sleep_until_end(&self) {
        self.sink.sleep_until_end();
    }

    /// Check if audio is currently playing
    pub fn is_playing(&self) -> bool {
        !self.sink.empty()
    }

    /// Stop playback
    pub fn stop(&self) {
        self.sink.stop();
        info!("Audio playback stopped");
    }

    /// Pause playback
    pub fn pause(&self) {
        self.sink.pause();
    }

    /// Resume playback
    pub fn play_paused(&self) {
        self.sink.play();
    }

    /// Set volume (0.0 to 1.0)
    pub fn set_volume(&self, volume: f32) {
        self.sink.set_volume(volume.clamp(0.0, 1.0));
    }

    /// Get current volume
    pub fn volume(&self) -> f32 {
        self.sink.volume()
    }
}

/// Record audio to a buffer for a specified duration
pub fn record_duration(duration_secs: f64, config: &AudioConfig) -> Result<AudioBuffer> {
    let input = AudioInput::new(config.clone())?;
    let sample_rate = input.sample_rate();
    let total_samples = (duration_secs * sample_rate as f64 * config.channels as f64) as usize;

    let all_samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity(total_samples)));
    let target_samples = Arc::new(Mutex::new(total_samples));

    let samples_left = target_samples.clone();
    let samples_collected = all_samples.clone();
    let stream = input.start(move |chunk| {
        if let Ok(mut remaining) = samples_left.lock() {
            let to_take = chunk.len().min(*remaining);
            if let Ok(mut samples) = samples_collected.lock() {
                samples.extend_from_slice(&chunk[..to_take]);
            }
            *remaining -= to_take;
        }
    })?;

    // Wait for recording to complete
    std::thread::sleep(std::time::Duration::from_secs_f64(duration_secs));

    input.stop(&stream)?;

    let samples = all_samples.lock()
        .map(|s| s.clone())
        .unwrap_or_default();
    Ok(AudioBuffer::from_samples(samples, sample_rate, config.channels))
}

/// Save audio buffer to WAV file
pub fn save_wav(buffer: &AudioBuffer, path: &std::path::Path) -> Result<()> {
    let spec = hound::WavSpec {
        channels: buffer.channels,
        sample_rate: buffer.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::create(path, spec)
        .context(format!("Failed to create WAV file: {:?}", path))?;

    for sample in buffer.samples_i16() {
        writer.write_sample(sample)?;
    }

    writer.finalize()?;
    info!("Saved {} samples to {:?}", buffer.len(), path);
    Ok(())
}

/// Load audio from WAV file
pub fn load_wav(path: &std::path::Path) -> Result<AudioBuffer> {
    let mut reader = hound::WavReader::open(path)
        .context(format!("Failed to open WAV file: {:?}", path))?;

    let spec = reader.spec();
    let samples: Vec<f32> = reader.samples::<i16>()
        .filter_map(|s| s.ok())
        .map(|s| s as f32 / i16::MAX as f32)
        .collect();

    info!("Loaded {} samples from {:?}", samples.len(), path);
    Ok(AudioBuffer::from_samples(samples, spec.sample_rate, spec.channels))
}

/// List available audio input devices
pub fn list_input_devices() -> Result<Vec<(String, cpal::Device)>> {
    let host = cpal::default_host();
    let mut devices = Vec::new();

    for device in host.input_devices()? {
        if let Ok(name) = device.name() {
            devices.push((name, device));
        }
    }

    Ok(devices)
}

/// List available audio output devices
pub fn list_output_devices() -> Result<Vec<(String, cpal::Device)>> {
    let host = cpal::default_host();
    let mut devices = Vec::new();

    for device in host.output_devices()? {
        if let Ok(name) = device.name() {
            devices.push((name, device));
        }
    }

    Ok(devices)
}

/// Get default input device info
pub fn get_default_input_info() -> Result<String> {
    let host = cpal::default_host();
    if let Some(device) = host.default_input_device() {
        let name = device.name()?;
        if let Ok(config) = device.default_input_config() {
            Ok(format!("{}: {:?}Hz", name, config.sample_rate()))
        } else {
            Ok(name)
        }
    } else {
        Ok("No default input device".to_string())
    }
}

/// Get default output device info
pub fn get_default_output_info() -> Result<String> {
    let host = cpal::default_host();
    if let Some(device) = host.default_output_device() {
        let name = device.name()?;
        if let Ok(config) = device.default_output_config() {
            Ok(format!("{}: {:?}Hz", name, config.sample_rate()))
        } else {
            Ok(name)
        }
    } else {
        Ok("No default output device".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_buffer() {
        let mut buffer = AudioBuffer::new(16000, 1);
        assert!(buffer.is_empty());

        buffer.push(&[0.5, -0.5, 0.0]);
        assert_eq!(buffer.len(), 3);

        let i16_samples = buffer.samples_i16();
        assert_eq!(i16_samples[0], (0.5 * i16::MAX as f32) as i16);
    }

    #[test]
    fn test_audio_config_default() {
        let config = AudioConfig::default();
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(config.channels, DEFAULT_CHANNELS);
    }

    #[test]
    fn test_buffer_resample() {
        let buffer = AudioBuffer::from_samples(
            vec![0.0, 0.5, -0.5, 0.0],
            16000,
            1
        );

        let resampled = buffer.resample(8000);
        assert_eq!(resampled.sample_rate, 8000);
        assert_eq!(resampled.channels, 1);
    }

    #[test]
    fn test_list_devices() {
        // This just tests that the function doesn't panic
        let _inputs = list_input_devices();
        let _outputs = list_output_devices();
    }
}
