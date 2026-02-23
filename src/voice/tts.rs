//! Text-to-Speech (TTS) Module
//!
//! Provides text-to-speech synthesis using Coqui TTS.
//! Supports multiple voices, languages, and real-time streaming.
//!
//! # Architecture
//!
//! ```text
//! Text Input → TTS Engine → Audio Output
//!                  ↓
//!            (Coqui TTS Model)
//! ```
//!
//! # Features
//!
//! - Local inference (no cloud required)
//! - Multiple voice models
//! - GPU acceleration support
//! - Audio streaming for low latency
//! - WAV and raw PCM output

use anyhow::{Result, Context, bail};
use serde::{Serialize, Deserialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, debug, warn};

use crate::voice::audio::{AudioOutput, AudioConfig};

/// Default sample rate for TTS output
pub const TTS_SAMPLE_RATE: u32 = 22050;

/// Default TTS model
pub const DEFAULT_TTS_MODEL: &str = "tts_models/en/ljspeech/tacotron2-DDC";

/// TTS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    /// Model name or path
    pub model_name: String,
    /// Sample rate for output
    pub sample_rate: u32,
    /// Use GPU acceleration
    pub use_gpu: bool,
    /// Voice speed (1.0 = normal)
    pub speed: f32,
    /// Output volume (0.0 - 1.0)
    pub volume: f32,
    /// Cache directory for models (serialized as string)
    #[serde(with = "path_serde")]
    pub model_cache_dir: PathBuf,
}

/// Helper module for PathBuf serialization
mod path_serde {
    use std::path::PathBuf;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(path: &PathBuf, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&path.to_string_lossy())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(PathBuf::from(s))
    }
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            model_name: DEFAULT_TTS_MODEL.to_string(),
            sample_rate: TTS_SAMPLE_RATE,
            use_gpu: false,
            speed: 1.0,
            volume: 0.8,
            model_cache_dir: dirs::cache_dir()
                .map(|d| d.join("my-agent/tts"))
                .unwrap_or_else(|| PathBuf::from("./tts_cache")),
        }
    }
}

impl TtsConfig {
    /// Create config with a specific model
    pub fn with_model(model_name: &str) -> Self {
        let mut config = Self::default();
        config.model_name = model_name.to_string();
        config
    }

    /// Enable GPU acceleration
    pub fn with_gpu(mut self) -> Self {
        self.use_gpu = true;
        self
    }

    /// Set playback speed
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed.clamp(0.5, 2.0);
        self
    }

    /// Set output volume
    pub fn with_volume(mut self, volume: f32) -> Self {
        self.volume = volume.clamp(0.0, 1.0);
        self
    }

    /// Load config from environment variables
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(model) = std::env::var("TTS_MODEL") {
            config.model_name = model;
        }

        if let Ok(gpu) = std::env::var("TTS_USE_GPU") {
            config.use_gpu = gpu.parse().unwrap_or(false);
        }

        if let Ok(speed) = std::env::var("TTS_SPEED") {
            config.speed = speed.parse().unwrap_or(1.0);
        }

        if let Ok(vol) = std::env::var("TTS_VOLUME") {
            config.volume = vol.parse().unwrap_or(0.8);
        }

        config
    }
}

/// Available TTS models
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsModel {
    /// English - LJSpeech Tacotron2
    LJSpeech,
    /// English - GlowTTS
    GlowTTS,
    /// Multilingual - YourTTS
    YourTTS,
    /// Multilingual - XTTS v2
    XttsV2,
}

impl TtsModel {
    /// Get model name for Coqui TTS
    pub fn model_name(&self) -> &'static str {
        match self {
            TtsModel::LJSpeech => "tts_models/en/ljspeech/tacotron2-DDC",
            TtsModel::GlowTTS => "tts_models/en/ljspeech/glow-tts",
            TtsModel::YourTTS => "tts_models/multilingual/multi-dataset/your_tts",
            TtsModel::XttsV2 => "tts_models/multilingual/multi-dataset/xtts_v2",
        }
    }

    /// Get model description
    pub fn description(&self) -> &'static str {
        match self {
            TtsModel::LJSpeech => "English female voice (default)",
            TtsModel::GlowTTS => "English female voice (GlowTTS)",
            TtsModel::YourTTS => "Multilingual voice cloning",
            TtsModel::XttsV2 => "High quality multilingual with voice cloning",
        }
    }

    /// Whether model supports voice cloning
    pub fn supports_cloning(&self) -> bool {
        matches!(self, TtsModel::YourTTS | TtsModel::XttsV2)
    }

    /// List all available models
    pub fn list_all() -> Vec<(String, String)> {
        vec![
            (TtsModel::LJSpeech.model_name().to_string(), TtsModel::LJSpeech.description().to_string()),
            (TtsModel::GlowTTS.model_name().to_string(), TtsModel::GlowTTS.description().to_string()),
            (TtsModel::YourTTS.model_name().to_string(), TtsModel::YourTTS.description().to_string()),
            (TtsModel::XttsV2.model_name().to_string(), TtsModel::XttsV2.description().to_string()),
        ]
    }
}

/// TTS Engine - synthesizes text to speech
pub struct TtsEngine {
    config: TtsConfig,
    #[cfg(feature = "coqui_tts")]
    synthesizer: Option<coqui_tts::Tts>,
    #[cfg(not(feature = "coqui_tts"))]
    synthesizer: Option<NoOpSynthesizer>,
}

/// Placeholder when Coqui TTS is not available
#[cfg(not(feature = "coqui_tts"))]
#[derive(Debug)]
pub struct NoOpSynthesizer;

#[cfg(not(feature = "coqui_tts"))]
impl NoOpSynthesizer {
    pub fn new() -> Result<Self> {
        bail!("Coqui TTS feature not enabled. Build with --features coqui_tts")
    }

    pub fn synthesize(&self, _text: &str) -> Result<Vec<f32>> {
        bail!("Coqui TTS feature not enabled. Build with --features coqui_tts")
    }
}

impl TtsEngine {
    /// Create a new TTS engine with default config
    pub fn new() -> Result<Self> {
        Self::with_config(TtsConfig::default())
    }

    /// Create TTS engine with custom config
    pub fn with_config(config: TtsConfig) -> Result<Self> {
        info!("Initializing TTS engine with model: {}", config.model_name);

        #[cfg(feature = "coqui_tts")]
        {
            // Initialize Coqui TTS synthesizer
            let synthesizer = Self::init_synthesizer(&config)?;

            Ok(Self {
                config,
                synthesizer: Some(synthesizer),
            })
        }

        #[cfg(not(feature = "coqui_tts"))]
        {
            warn!("Coqui TTS feature not enabled. TTS will be unavailable.");
            Ok(Self {
                config,
                synthesizer: None,
            })
        }
    }

    /// Initialize the synthesizer (only with coqui_tts feature)
    #[cfg(feature = "coqui_tts")]
    fn init_synthesizer(config: &TtsConfig) -> Result<coqui_tts::Tts> {
        use coqui_tts::Tts;

        // Ensure model cache directory exists
        std::fs::create_dir_all(&config.model_cache_dir)
            .context("Failed to create TTS cache directory")?;

        // Initialize TTS with model
        let tts = Tts::new(&config.model_name)
            .context("Failed to initialize Coqui TTS")?;

        info!("TTS engine initialized successfully");
        Ok(tts)
    }

    /// Synthesize text to audio samples
    pub fn synthesize(&self, text: &str) -> Result<TtsResult> {
        if text.is_empty() {
            bail!("Cannot synthesize empty text");
        }

        debug!("Synthesizing text: {}...", &text[..text.len().min(50)]);

        #[cfg(feature = "coqui_tts")]
        {
            if let Some(ref synth) = self.synthesizer {
                let start = std::time::Instant::now();

                // Synthesize to raw audio
                let audio = synth.tts(text, None)
                    .context("TTS synthesis failed")?;

                // Convert to f32 samples (assuming i16 output from Coqui)
                let samples: Vec<f32> = audio.iter()
                    .map(|&s| (s as f32 / 32768.0).clamp(-1.0, 1.0))
                    .collect();

                // Apply volume adjustment
                let samples: Vec<f32> = samples.iter()
                    .map(|&s| s * self.config.volume)
                    .collect();

                // Apply speed adjustment if needed
                let samples = if (self.config.speed - 1.0).abs() > 0.01 {
                    Self::change_speed(&samples, self.config.speed, self.config.sample_rate)
                } else {
                    samples
                };

                let duration = start.elapsed();
                info!("Synthesized {} chars in {:?}", text.len(), duration);

                let duration_secs = samples.len() as f32 / self.config.sample_rate as f32;

                Ok(TtsResult {
                    samples,
                    sample_rate: self.config.sample_rate,
                    duration_secs,
                    text: text.to_string(),
                })
            } else {
                bail!("TTS synthesizer not initialized")
            }
        }

        #[cfg(not(feature = "coqui_tts"))]
        {
            // Fallback: generate placeholder audio
            warn!("Coqui TTS not available, returning silence");
            let duration_secs = (text.len() as f32 * 0.05).clamp(0.5, 5.0);
            let num_samples = (duration_secs * self.config.sample_rate as f32) as usize;

            Ok(TtsResult {
                samples: vec![0.0f32; num_samples],
                sample_rate: self.config.sample_rate,
                duration_secs,
                text: text.to_string(),
            })
        }
    }

    /// Synthesize text and play immediately
    pub fn speak(&self, text: &str) -> Result<()> {
        let result = self.synthesize(text)?;
        result.play()
    }

    /// Synthesize text and save to WAV file
    pub fn save_to_file(&self, text: &str, path: &Path) -> Result<()> {
        let result = self.synthesize(text)?;
        result.save_to_file(path)
    }

    /// Change audio speed using resampling
    #[cfg(feature = "coqui_tts")]
    fn change_speed(samples: &[f32], speed: f32, sample_rate: u32) -> Vec<f32> {
        use rubato::{Resampler, PolynomialDegree};

        if speed <= 0.0 || (speed - 1.0).abs() < 0.001 {
            return samples.to_vec();
        }

        let new_rate = (sample_rate as f32 * speed) as u64;
        let params = rubato::InterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: rubato::InterpolationType::Linear,
            oversampling_factor: 256,
            window: rubato::WindowFunction::BlackmanHarris2,
        };

        let mut resampler = rubato::SincFixedOut::new(
            sample_rate as f64 / new_rate as f64,
            2.0,
            params,
            samples.len(),
            1,
        ).unwrap_or_else(|_| {
            // Fallback: just return original
            return rubato::SincFixedOut::new(
                1.0,
                2.0,
                params,
                samples.len(),
                1,
            ).unwrap()
        });

        let input = vec![samples.to_vec()];
        match resampler.process(&input, None) {
            Ok(output) => output[0].clone(),
            Err(_) => samples.to_vec(),
        }
    }

    #[cfg(not(feature = "coqui_tts"))]
    fn change_speed(samples: &[f32], _speed: f32, _sample_rate: u32) -> Vec<f32> {
        samples.to_vec()
    }

    /// Get current config
    pub fn config(&self) -> &TtsConfig {
        &self.config
    }

    /// Check if TTS is available
    pub fn is_available(&self) -> bool {
        self.synthesizer.is_some()
    }
}

impl Default for TtsEngine {
    fn default() -> Self {
        Self::new().unwrap_or_else(|_| {
            Self {
                config: TtsConfig::default(),
                synthesizer: None,
            }
        })
    }
}

/// TTS synthesis result
pub struct TtsResult {
    /// Raw audio samples (f32, -1.0 to 1.0)
    pub samples: Vec<f32>,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Duration in seconds
    pub duration_secs: f32,
    /// Original text
    pub text: String,
}

impl TtsResult {
    /// Play the audio through speakers
    pub fn play(&self) -> Result<()> {
        use crate::voice::audio::AudioBuffer;

        let config = AudioConfig::default();
        let output = AudioOutput::new(config)?;

        // Create AudioBuffer which handles resampling automatically
        let buffer = AudioBuffer::from_samples(
            self.samples.clone(),
            self.sample_rate,
            1, // mono
        );

        output.play_buffer(&buffer)?;

        // Wait for playback to complete
        output.sleep_until_end();

        Ok(())
    }

    /// Save to WAV file
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        use hound::{WavWriter, WavSpec, SampleFormat};

        let spec = WavSpec {
            channels: 1,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };

        let mut writer = WavWriter::create(path, spec)
            .context("Failed to create WAV file")?;

        for &sample in &self.samples {
            let i16_sample = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
            writer.write_sample(i16_sample)?;
        }

        writer.finalize()?;
        info!("Saved TTS audio to: {}", path.display());

        Ok(())
    }

    /// Get audio as bytes (16-bit PCM)
    pub fn to_pcm_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.samples.len() * 2);
        for &sample in &self.samples {
            let i16_sample = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
            bytes.extend_from_slice(&i16_sample.to_le_bytes());
        }
        bytes
    }

    /// Get audio as base64 encoded string
    pub fn to_base64(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(self.to_pcm_bytes())
    }
}

/// Streaming TTS for real-time playback
pub struct StreamingTts {
    engine: Arc<TtsEngine>,
    buffer: Vec<f32>,
}

impl StreamingTts {
    /// Create new streaming TTS
    pub fn new(engine: Arc<TtsEngine>) -> Self {
        Self {
            engine,
            buffer: Vec::new(),
        }
    }

    /// Synthesize a chunk of text (for sentence-by-sentence streaming)
    pub fn synthesize_chunk(&mut self, text: &str) -> Result<Vec<f32>> {
        let result = self.engine.synthesize(text)?;
        Ok(result.samples)
    }

    /// Speak text sentence by sentence for lower latency
    pub fn speak_streaming(&self, text: &str) -> Result<()> {
        // Split text into sentences (collect into owned Strings for thread safety)
        let sentences: Vec<String> = text
            .split(|c| c == '.' || c == '!' || c == '?')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        if sentences.is_empty() {
            return Ok(());
        }

        // Spawn playback thread
        let engine = self.engine.clone();
        std::thread::spawn(move || {
            for sentence in sentences {
                let full_sentence = format!("{}.", sentence);
                if let Err(e) = engine.speak(&full_sentence) {
                    warn!("TTS synthesis error: {}", e);
                }
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tts_config_default() {
        let config = TtsConfig::default();
        assert_eq!(config.model_name, DEFAULT_TTS_MODEL);
        assert_eq!(config.sample_rate, TTS_SAMPLE_RATE);
        assert!(!config.use_gpu);
        assert_eq!(config.speed, 1.0);
    }

    #[test]
    fn test_tts_config_with_model() {
        let config = TtsConfig::with_model("tts_models/en/ljspeech/glow-tts");
        assert_eq!(config.model_name, "tts_models/en/ljspeech/glow-tts");
    }

    #[test]
    fn test_tts_config_builder() {
        let config = TtsConfig::default()
            .with_gpu()
            .with_speed(1.5)
            .with_volume(0.5);

        assert!(config.use_gpu);
        assert_eq!(config.speed, 1.5);
        assert_eq!(config.volume, 0.5);
    }

    #[test]
    fn test_tts_model_list() {
        let models = TtsModel::list_all();
        assert!(!models.is_empty());
        assert!(models.iter().any(|(n, _)| n.contains("ljspeech")));
    }

    #[test]
    fn test_tts_model_properties() {
        assert!(TtsModel::XttsV2.supports_cloning());
        assert!(TtsModel::YourTTS.supports_cloning());
        assert!(!TtsModel::LJSpeech.supports_cloning());
    }

    #[test]
    fn test_tts_result_duration() {
        let sample_rate: u32 = 22050;
        let num_samples = sample_rate as usize * 2; // 2 seconds
        let samples = vec![0.0f32; num_samples];

        let result = TtsResult {
            samples,
            sample_rate,
            duration_secs: 2.0,
            text: "Hello".to_string(),
        };

        assert_eq!(result.duration_secs, 2.0);
        assert_eq!(result.sample_rate, sample_rate);
    }

    #[test]
    fn test_tts_result_to_pcm_bytes() {
        let samples = vec![0.0f32, 1.0f32, -1.0f32];
        let result = TtsResult {
            samples,
            sample_rate: 22050,
            duration_secs: 0.1,
            text: "Test".to_string(),
        };

        let bytes = result.to_pcm_bytes();
        assert_eq!(bytes.len(), 6); // 3 samples * 2 bytes
    }

    #[test]
    fn test_streaming_tts_sentence_split() {
        let text = "Hello world. This is a test. How are you?";
        let sentences: Vec<&str> = text
            .split(|c| c == '.' || c == '!' || c == '?')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        assert_eq!(sentences.len(), 3);
        assert_eq!(sentences[0], "Hello world");
        assert_eq!(sentences[1], "This is a test");
        assert_eq!(sentences[2], "How are you");
    }

    #[test]
    fn test_tts_config_from_env() {
        // Set env vars
        std::env::set_var("TTS_MODEL", "test-model");
        std::env::set_var("TTS_USE_GPU", "true");
        std::env::set_var("TTS_SPEED", "1.5");
        std::env::set_var("TTS_VOLUME", "0.7");

        let config = TtsConfig::from_env();

        assert_eq!(config.model_name, "test-model");
        assert!(config.use_gpu);
        assert_eq!(config.speed, 1.5);
        assert_eq!(config.volume, 0.7);

        // Clean up
        std::env::remove_var("TTS_MODEL");
        std::env::remove_var("TTS_USE_GPU");
        std::env::remove_var("TTS_SPEED");
        std::env::remove_var("TTS_VOLUME");
    }
}

/// Convenience function to synthesize text to speech
///
/// Returns audio bytes (WAV format) for the synthesized speech.
pub async fn synthesize(text: &str) -> Result<Vec<u8>> {
    // For now, return empty audio. In a real implementation, this would:
    // 1. Initialize Piper or Coqui TTS
    // 2. Synthesize the text
    // 3. Return WAV audio bytes
    
    // Placeholder: Return empty audio with WAV header
    // In production, this would actually synthesize speech
    Ok(vec![])
}
