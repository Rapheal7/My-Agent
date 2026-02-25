//! Silero VAD (Voice Activity Detection) using ONNX Runtime
//!
//! ML-based voice activity detection using the Silero VAD v5 ONNX model.
//! Much more accurate than energy-based VAD for handling noise, breathing,
//! and non-speech sounds.
//!
//! The model is auto-downloaded (~2MB) on first use.

use anyhow::{Result, Context};
use ort::session::Session;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::{info, debug, warn};

const SILERO_VAD_URL: &str = "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx";
const MODEL_FILENAME: &str = "silero_vad.onnx";
const SAMPLE_RATE: i64 = 16000;
const WINDOW_SIZE: usize = 512; // 32ms at 16kHz

/// Events emitted by the Silero VAD
#[derive(Debug, Clone, PartialEq)]
pub enum VadEvent {
    /// No speech detected
    Silence,
    /// Speech just started
    SpeechStart,
    /// Ongoing speech
    Speaking,
    /// Speech just ended
    SpeechEnd,
    /// Mid-speech pause (for backchanneling)
    Pause { duration: Duration },
}

/// Silero VAD wrapper using ONNX Runtime
pub struct SileroVad {
    session: Session,
    /// Hidden state tensor (2, 1, 128) - persists across calls
    state: Vec<f32>,
    /// Speech probability threshold
    threshold: f32,
    /// Minimum speech duration before confirming
    min_speech_ms: u64,
    /// Minimum silence duration before ending speech
    min_silence_ms: u64,
    /// Current speaking state
    is_speaking: bool,
    /// When speech started
    speech_start: Option<Instant>,
    /// When silence started (during speech)
    silence_start: Option<Instant>,
    /// Last speech time for pause detection
    last_speech_time: Option<Instant>,
}

impl SileroVad {
    /// Create a new SileroVad, downloading the model if needed
    pub fn new() -> Result<Self> {
        Self::with_threshold(0.5)
    }

    /// Create with a custom threshold (0.0-1.0)
    pub fn with_threshold(threshold: f32) -> Result<Self> {
        let model_path = Self::ensure_model()?;

        let session = Session::builder()?
            .with_intra_threads(1)?
            .commit_from_file(&model_path)
            .context("Failed to create ONNX session for Silero VAD")?;

        info!("Silero VAD loaded from {}", model_path.display());

        Ok(Self {
            session,
            state: vec![0.0f32; 2 * 1 * 128], // h and c states
            threshold,
            min_speech_ms: 250,
            min_silence_ms: 300,
            is_speaking: false,
            speech_start: None,
            silence_start: None,
            last_speech_time: None,
        })
    }

    /// Ensure the ONNX model file exists, downloading if needed
    fn ensure_model() -> Result<PathBuf> {
        let model_dir = crate::config::data_dir()?.join("models");
        std::fs::create_dir_all(&model_dir)
            .context("Failed to create models directory")?;

        let model_path = model_dir.join(MODEL_FILENAME);
        if model_path.exists() {
            return Ok(model_path);
        }

        info!("Downloading Silero VAD model to {}", model_path.display());

        let response = reqwest::blocking::get(SILERO_VAD_URL)
            .context("Failed to download Silero VAD model")?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Failed to download Silero VAD model: HTTP {}",
                response.status()
            ));
        }

        let bytes = response.bytes()
            .context("Failed to read Silero VAD model bytes")?;

        std::fs::write(&model_path, &bytes)
            .context("Failed to save Silero VAD model")?;

        info!("Silero VAD model downloaded ({} bytes)", bytes.len());
        Ok(model_path)
    }

    /// Process a chunk of audio samples (should be WINDOW_SIZE = 512 samples)
    ///
    /// Audio should be f32 normalized to [-1.0, 1.0] at 16kHz.
    pub fn process_chunk(&mut self, audio: &[f32]) -> VadEvent {
        let prob = match self.run_inference(audio) {
            Ok(p) => p,
            Err(e) => {
                warn!("VAD inference error: {}", e);
                return VadEvent::Silence;
            }
        };

        let now = Instant::now();
        let is_speech = prob >= self.threshold;

        if is_speech {
            self.last_speech_time = Some(now);

            if !self.is_speaking {
                // Potential speech start
                match self.speech_start {
                    None => {
                        self.speech_start = Some(now);
                        self.silence_start = None;
                        VadEvent::Silence // Wait for min_speech_ms
                    }
                    Some(start) => {
                        if now.duration_since(start).as_millis() as u64 >= self.min_speech_ms {
                            self.is_speaking = true;
                            self.silence_start = None;
                            VadEvent::SpeechStart
                        } else {
                            VadEvent::Silence // Still warming up
                        }
                    }
                }
            } else {
                self.silence_start = None;
                VadEvent::Speaking
            }
        } else {
            // No speech in this chunk
            self.speech_start = None;

            if self.is_speaking {
                // We were speaking, now silence
                match self.silence_start {
                    None => {
                        self.silence_start = Some(now);
                        // Check for mid-speech pause
                        if let Some(last_speech) = self.last_speech_time {
                            let pause_dur = now.duration_since(last_speech);
                            if pause_dur.as_millis() > 100 {
                                return VadEvent::Pause { duration: pause_dur };
                            }
                        }
                        VadEvent::Speaking // Brief silence, don't cut off yet
                    }
                    Some(silence_start) => {
                        let silence_duration = now.duration_since(silence_start);
                        if silence_duration.as_millis() as u64 >= self.min_silence_ms {
                            self.is_speaking = false;
                            self.silence_start = None;
                            VadEvent::SpeechEnd
                        } else {
                            // Mid-speech pause
                            VadEvent::Pause { duration: silence_duration }
                        }
                    }
                }
            } else {
                VadEvent::Silence
            }
        }
    }

    /// Run inference on the ONNX model
    fn run_inference(&mut self, audio: &[f32]) -> Result<f32> {
        use ort::value::Value;

        // Prepare input tensor [1, window_size]
        let audio_len = audio.len();
        let input_data: Vec<f32> = audio.to_vec();
        let input = Value::from_array(([1usize, audio_len], input_data))?;

        // State tensor [2, 1, 128]
        let state_data: Vec<f32> = self.state.clone();
        let state = Value::from_array(([2usize, 1usize, 128usize], state_data))?;

        // Sample rate tensor [1]
        let sr = Value::from_array(([1usize], vec![SAMPLE_RATE]))?;

        // Run inference
        let outputs = self.session.run(ort::inputs![input, state, sr])?;

        // Extract probability (output 0 is the speech probability)
        let (_prob_shape, prob_data) = outputs[0].try_extract_tensor::<f32>()?;
        let prob = if !prob_data.is_empty() { prob_data[0] } else { 0.0 };

        // Update state from output (output 1 is the new state)
        let (_state_shape, new_state_data) = outputs[1].try_extract_tensor::<f32>()?;
        if new_state_data.len() == self.state.len() {
            self.state.copy_from_slice(new_state_data);
        }

        Ok(prob)
    }

    /// Reset hidden state (call between sessions)
    pub fn reset(&mut self) {
        self.state.fill(0.0);
        self.is_speaking = false;
        self.speech_start = None;
        self.silence_start = None;
        self.last_speech_time = None;
        debug!("Silero VAD state reset");
    }

    /// Get the required chunk size in samples
    pub fn window_size(&self) -> usize {
        WINDOW_SIZE
    }

    /// Check if currently in speaking state
    pub fn is_speaking(&self) -> bool {
        self.is_speaking
    }
}

/// Convert i16 PCM samples to f32 normalized [-1.0, 1.0]
pub fn i16_to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|&s| s as f32 / 32768.0).collect()
}

/// Convert f32 normalized samples to i16 PCM
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_i16_to_f32_conversion() {
        let samples = vec![0i16, 32767, -32768];
        let converted = i16_to_f32(&samples);
        assert!((converted[0] - 0.0).abs() < 0.001);
        assert!((converted[1] - 1.0).abs() < 0.001);
        assert!((converted[2] - (-1.0)).abs() < 0.001);
    }

    #[test]
    fn test_f32_to_i16_conversion() {
        let samples = vec![0.0f32, 1.0, -1.0];
        let converted = f32_to_i16(&samples);
        assert_eq!(converted[0], 0);
        assert_eq!(converted[1], 32767);
        assert_eq!(converted[2], -32767);
    }

    #[test]
    fn test_vad_event_equality() {
        assert_eq!(VadEvent::Silence, VadEvent::Silence);
        assert_eq!(VadEvent::SpeechStart, VadEvent::SpeechStart);
        assert_ne!(VadEvent::Silence, VadEvent::SpeechStart);
    }
}
