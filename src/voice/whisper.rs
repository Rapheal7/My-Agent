//! Whisper Speech-to-Text (STT) Module
//!
//! Provides local speech recognition using OpenAI's Whisper model.
//! Uses whisper-rs for Rust bindings to the Whisper C++ library.
//!
//! # Model Sizes
//!
//! | Model  | Size  | VRAM/RAM | Speed | Accuracy | Best For |
//! |--------|-------|----------|-------|----------|----------|
//! | tiny   | 39 MB | ~1 GB    | Fastest | Low | Quick tests |
//! | base   | 74 MB | ~1 GB    | Fast | Good | Real-time |
//! | small  | 244 MB| ~2 GB    | Medium| Better | General use |
//! | medium | 769 MB| ~5 GB    | Slow | High | Accuracy |
//! | large  | 1550 MB|~10 GB  | Slowest | Best | Production |
//!
//! # Architecture
//!
//! ```text
//! Audio Input → Preprocessing → Whisper Model → Text Output
//!                   ↓
//!            (Resample to 16kHz, mono)
//! ```

use anyhow::{Result, Context, bail};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::{info, debug, warn};

/// Default sample rate required by Whisper
pub const WHISPER_SAMPLE_RATE: i32 = 16000;

/// Default number of threads for Whisper inference
pub const DEFAULT_WHISPER_THREADS: i32 = 4;

/// Default language (auto-detect if None)
pub const DEFAULT_LANGUAGE: Option<&str> = None;

/// Whisper model configuration
#[derive(Debug, Clone)]
pub struct WhisperConfig {
    /// Path to the model file or directory containing models
    pub model_path: PathBuf,
    /// Model size/type (tiny, base, small, medium, large)
    pub model_size: String,
    /// Number of threads for inference
    pub threads: i32,
    /// Language code (e.g., "en", "es") or None for auto-detect
    pub language: Option<String>,
    /// Enable translation to English
    pub translate: bool,
    /// Enable timestamps in output
    pub timestamps: bool,
    /// Enable verbose output
    pub verbose: bool,
    /// Beam search width (higher = better quality, slower)
    pub beam_size: i32,
    /// Audio context size (default 1500)
    pub audio_ctx: i32,
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            model_path: PathBuf::from("models/whisper"),
            model_size: "base".to_string(),
            threads: DEFAULT_WHISPER_THREADS,
            language: DEFAULT_LANGUAGE.map(|s| s.to_string()),
            translate: false,
            timestamps: false,
            verbose: false,
            beam_size: 5,
            audio_ctx: 1500,
        }
    }
}

impl WhisperConfig {
    /// Create config with a specific model size
    pub fn with_model_size(size: &str) -> Self {
        Self {
            model_size: size.to_string(),
            ..Default::default()
        }
    }

    /// Get the expected model filename
    pub fn model_filename(&self) -> String {
        format!("ggml-{}.bin", self.model_size)
    }

    /// Get full path to model file
    pub fn full_model_path(&self) -> PathBuf {
        self.model_path.join(self.model_filename())
    }

    /// Check if model file exists
    pub fn model_exists(&self) -> bool {
        self.full_model_path().exists()
    }

    /// Load from environment/config
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(path) = std::env::var("WHISPER_MODEL_PATH") {
            config.model_path = PathBuf::from(path);
        }

        if let Ok(size) = std::env::var("WHISPER_MODEL_SIZE") {
            config.model_size = size;
        }

        if let Ok(threads) = std::env::var("WHISPER_THREADS") {
            if let Ok(t) = threads.parse() {
                config.threads = t;
            }
        }

        if let Ok(lang) = std::env::var("WHISPER_LANGUAGE") {
            if lang.to_lowercase() == "auto" {
                config.language = None;
            } else {
                config.language = Some(lang);
            }
        }

        config
    }
}

/// Whisper model download information
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub size: &'static str,
    pub url: &'static str,
    pub description: &'static str,
    pub approximate_size_mb: usize,
}

/// Available Whisper models
pub const AVAILABLE_MODELS: &[ModelInfo] = &[
    ModelInfo {
        size: "tiny",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        description: "Fastest, lowest accuracy - good for testing",
        approximate_size_mb: 39,
    },
    ModelInfo {
        size: "base",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        description: "Fast with good accuracy - recommended for real-time",
        approximate_size_mb: 74,
    },
    ModelInfo {
        size: "small",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        description: "Balanced speed and accuracy",
        approximate_size_mb: 244,
    },
    ModelInfo {
        size: "medium",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
        description: "High accuracy, slower",
        approximate_size_mb: 769,
    },
    ModelInfo {
        size: "large-v3",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
        description: "Best accuracy, slowest - production use",
        approximate_size_mb: 1550,
    },
];

/// Get model info by size name
pub fn get_model_info(size: &str) -> Option<&'static ModelInfo> {
    AVAILABLE_MODELS.iter().find(|m| m.size == size)
}

/// Transcription result from Whisper
#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    /// Full transcribed text
    pub text: String,
    /// Individual segments with timestamps
    pub segments: Vec<TranscriptionSegment>,
    /// Detected language (if auto-detect enabled)
    pub language: Option<String>,
    /// Processing duration
    pub processing_time_secs: f64,
}

/// A single transcription segment
#[derive(Debug, Clone)]
pub struct TranscriptionSegment {
    /// Segment text
    pub text: String,
    /// Start time in seconds
    pub start_secs: f64,
    /// End time in seconds
    pub end_secs: f64,
    /// Confidence/probability (0.0-1.0)
    pub probability: f32,
}

/// Whisper STT engine
pub struct WhisperEngine {
    config: WhisperConfig,
    #[cfg(feature = "whisper")]
    context: Option<whisper_rs::WhisperContext>,
    #[cfg(not(feature = "whisper"))]
    context: Option<NoWhisperContext>,
}

/// Placeholder context when whisper feature is not enabled
#[cfg(not(feature = "whisper"))]
#[derive(Debug)]
pub struct NoWhisperContext;

impl WhisperEngine {
    /// Create a new Whisper engine with default config
    pub fn new() -> Result<Self> {
        Self::with_config(WhisperConfig::default())
    }

    /// Create a new Whisper engine with custom config
    pub fn with_config(config: WhisperConfig) -> Result<Self> {
        info!("Creating Whisper engine with config: {:?}", config);

        #[cfg(not(feature = "whisper"))]
        {
            bail!("Whisper feature not enabled. Build with --features whisper")
        }

        #[cfg(feature = "whisper")]
        {
            if !config.model_exists() {
                bail!(
                    "Whisper model not found at: {}. Run download_model() first or set WHISPER_MODEL_PATH",
                    config.full_model_path().display()
                );
            }

            let ctx = Self::load_model(&config)?;

            Ok(Self {
                config,
                context: Some(ctx),
            })
        }
    }

    /// Try to create engine, returning None if model not available
    pub fn try_new() -> Option<Self> {
        let config = WhisperConfig::from_env();
        if !config.model_exists() {
            warn!("Whisper model not found at: {}", config.full_model_path().display());
            return None;
        }

        match Self::with_config(config) {
            Ok(engine) => Some(engine),
            Err(e) => {
                warn!("Failed to create Whisper engine: {}", e);
                None
            }
        }
    }

    #[cfg(feature = "whisper")]
    fn load_model(config: &WhisperConfig) -> Result<whisper_rs::WhisperContext> {
        use whisper_rs::WhisperContextParameters;

        let model_path = config.full_model_path();
        info!("Loading Whisper model from: {}", model_path.display());

        let start = std::time::Instant::now();

        let ctx_params = WhisperContextParameters::default();
        let ctx = whisper_rs::WhisperContext::new_with_params(
            model_path.to_str().context("Invalid model path")?,
            ctx_params,
        ).context("Failed to load Whisper model")?;

        info!("Whisper model loaded in {:.2}s", start.elapsed().as_secs_f64());

        Ok(ctx)
    }

    /// Transcribe audio samples to text
    ///
    /// Audio must be 16kHz, mono, f32 samples
    pub fn transcribe(&self, audio_samples: &[f32]) -> Result<TranscriptionResult> {
        #[cfg(not(feature = "whisper"))]
        {
            let _ = audio_samples;
            bail!("Whisper feature not enabled. Build with --features whisper")
        }

        #[cfg(feature = "whisper")]
        {
            let ctx = self.context.as_ref().context("Whisper model not loaded")?;
            let start_time = std::time::Instant::now();

            // Create state
            let mut state = ctx.create_state().context("Failed to create Whisper state")?;

            // Setup parameters
            let mut params = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });

            params.set_n_threads(self.config.threads);
            params.set_translate(self.config.translate);
            params.set_print_special(false);
            params.set_print_progress(self.config.verbose);
            params.set_print_realtime(false);
            params.set_print_timestamps(self.config.verbose);

            // Set language if specified
            if let Some(ref lang) = self.config.language {
                params.set_language(Some(lang));
            } else {
                params.set_language(None); // Auto-detect
            }

            // Set beam size if using beam search
            if self.config.beam_size > 1 {
                params.set_sampling_strategy(whisper_rs::SamplingStrategy::BeamSearch {
                    beam_size: self.config.beam_size,
                    patience: 1.0,
                });
            }

            debug!("Running Whisper inference on {} samples", audio_samples.len());

            // Run inference
            state.full(params, audio_samples)
                .context("Whisper inference failed")?;

            // Extract results
            let num_segments = state.full_n_segments().context("Failed to get segment count")?;
            let mut segments = Vec::with_capacity(num_segments as usize);
            let mut full_text = String::new();

            for i in 0..num_segments {
                let text = state.full_get_segment_text(i)
                    .context("Failed to get segment text")?;

                let start = state.full_get_segment_t0(i)
                    .context("Failed to get segment start")? as f64 / 100.0;

                let end = state.full_get_segment_t1(i)
                    .context("Failed to get segment end")? as f64 / 100.0;

                // Try to get probability if available
                let prob = state.full_get_segment_prob(i).unwrap_or(0.0);

                segments.push(TranscriptionSegment {
                    text: text.trim().to_string(),
                    start_secs: start,
                    end_secs: end,
                    probability: prob,
                });

                full_text.push_str(&text);
                full_text.push(' ');
            }

            // Get detected language
            let lang_id = state.full_lang_id().ok();
            let language = lang_id.map(|id| {
                // Convert language ID to code
                whisper_lang_str(id).to_string()
            });

            let processing_time = start_time.elapsed().as_secs_f64();

            info!(
                "Transcription complete: {} segments in {:.2}s",
                segments.len(),
                processing_time
            );

            Ok(TranscriptionResult {
                text: full_text.trim().to_string(),
                segments,
                language,
                processing_time_secs: processing_time,
            })
        }
    }

    /// Transcribe from 16-bit PCM samples (converts to f32)
    pub fn transcribe_i16(&self, samples: &[i16]) -> Result<TranscriptionResult> {
        let f32_samples: Vec<f32> = samples.iter()
            .map(|&s| s as f32 / i16::MAX as f32)
            .collect();
        self.transcribe(&f32_samples)
    }

    /// Get the configured model size
    pub fn model_size(&self) -> &str {
        &self.config.model_size
    }

    /// Check if engine is ready
    pub fn is_ready(&self) -> bool {
        self.context.is_some()
    }
}

impl Default for WhisperEngine {
    fn default() -> Self {
        Self::new().unwrap_or_else(|_| Self {
            config: WhisperConfig::default(),
            context: None,
        })
    }
}

/// Download a Whisper model
pub async fn download_model(model_size: &str, output_dir: &Path) -> Result<PathBuf> {
    let model_info = get_model_info(model_size)
        .context(format!("Unknown model size: {}", model_size))?;

    std::fs::create_dir_all(output_dir)
        .context("Failed to create model directory")?;

    let output_path = output_dir.join(format!("ggml-{}.bin", model_size));

    if output_path.exists() {
        info!("Model already exists at: {}", output_path.display());
        return Ok(output_path);
    }

    info!(
        "Downloading Whisper {} model (~{} MB)...",
        model_size,
        model_info.approximate_size_mb
    );

    let client = reqwest::Client::new();
    let response = client.get(model_info.url)
        .send()
        .await
        .context("Failed to download model")?;

    if !response.status().is_success() {
        bail!("Download failed with status: {}", response.status());
    }

    let bytes = response.bytes()
        .await
        .context("Failed to read model data")?;

    std::fs::write(&output_path, bytes)
        .context("Failed to write model file")?;

    info!("Model downloaded to: {}", output_path.display());

    Ok(output_path)
}

/// List available models with descriptions
pub fn list_available_models() {
    println!("Available Whisper models:");
    println!();

    for model in AVAILABLE_MODELS {
        println!("  {:12} ~{:4} MB - {}",
            model.size,
            model.approximate_size_mb,
            model.description
        );
    }
}

/// Preprocess audio for Whisper
///
/// - Resample to 16kHz if needed
/// - Convert to mono if stereo
/// - Normalize levels
pub fn preprocess_audio(
    samples: &[f32],
    input_sample_rate: u32,
    input_channels: u16,
) -> Vec<f32> {
    debug!(
        "Preprocessing audio: {}Hz, {}ch, {} samples",
        input_sample_rate,
        input_channels,
        samples.len()
    );

    // Convert to mono if needed
    let mono_samples = if input_channels == 2 {
        samples.chunks(2)
            .map(|chunk| {
                if chunk.len() == 2 {
                    (chunk[0] + chunk[1]) / 2.0
                } else {
                    chunk[0]
                }
            })
            .collect::<Vec<_>>()
    } else {
        samples.to_vec()
    };

    // Resample to 16kHz if needed
    let resampled = if input_sample_rate != WHISPER_SAMPLE_RATE as u32 {
        resample_linear(&mono_samples, input_sample_rate, WHISPER_SAMPLE_RATE as u32)
    } else {
        mono_samples
    };

    // Normalize to prevent clipping
    let max_amplitude = resampled.iter()
        .map(|&s| s.abs())
        .fold(0.0f32, f32::max);

    if max_amplitude > 1.0 {
        resampled.iter()
            .map(|&s| s / max_amplitude)
            .collect()
    } else {
        resampled
    }
}

/// Simple linear resampling
fn resample_linear(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }

    let ratio = to_rate as f64 / from_rate as f64;
    let new_len = (samples.len() as f64 * ratio) as usize;
    let mut result = Vec::with_capacity(new_len);

    for i in 0..new_len {
        let src_idx = i as f64 / ratio;
        let idx_floor = src_idx.floor() as usize;
        let idx_ceil = (idx_floor + 1).min(samples.len() - 1);
        let frac = src_idx - idx_floor as f64;

        let val = samples[idx_floor] * (1.0 - frac as f32)
                + samples[idx_ceil] * frac as f32;
        result.push(val);
    }

    result
}

#[cfg(feature = "whisper")]
fn whisper_lang_str(id: i32) -> &'static str {
    // Common language IDs
    match id {
        0 => "en",
        1 => "zh",
        2 => "de",
        3 => "es",
        4 => "ru",
        5 => "ko",
        6 => "fr",
        7 => "ja",
        8 => "pt",
        9 => "tr",
        10 => "pl",
        11 => "ca",
        12 => "nl",
        13 => "ar",
        14 => "sv",
        15 => "it",
        16 => "id",
        17 => "hi",
        18 => "fi",
        19 => "vi",
        20 => "he",
        21 => "uk",
        22 => "el",
        23 => "ms",
        24 => "cs",
        25 => "ro",
        26 => "da",
        27 => "hu",
        28 => "ta",
        29 => "no",
        30 => "th",
        31 => "ur",
        32 => "hr",
        33 => "bg",
        34 => "lt",
        35 => "la",
        36 => "mi",
        37 => "ml",
        38 => "cy",
        39 => "sk",
        40 => "te",
        41 => "fa",
        42 => "lv",
        43 => "bn",
        44 => "sr",
        45 => "az",
        46 => "sl",
        47 => "kn",
        48 => "et",
        49 => "mk",
        50 => "br",
        51 => "eu",
        52 => "is",
        53 => "hy",
        54 => "ne",
        55 => "mn",
        56 => "bs",
        57 => "kk",
        58 => "sq",
        59 => "sw",
        60 => "gl",
        61 => "mr",
        62 => "pa",
        63 => "si",
        64 => "km",
        65 => "sn",
        66 => "yo",
        67 => "so",
        68 => "af",
        69 => "oc",
        70 => "ka",
        71 => "be",
        72 => "tg",
        73 => "sd",
        74 => "gu",
        75 => "am",
        76 => "yi",
        77 => "lo",
        78 => "uz",
        79 => "fo",
        80 => "ht",
        81 => "ps",
        82 => "tk",
        83 => "nn",
        84 => "mt",
        85 => "sa",
        86 => "lb",
        87 => "my",
        88 => "bo",
        89 => "tl",
        90 => "mg",
        91 => "as",
        92 => "tt",
        93 => "haw",
        94 => "ln",
        95 => "ha",
        96 => "ba",
        97 => "jw",
        98 => "su",
        99 => "yue",
        _ => "unknown",
    }
}

/// Real-time speech-to-text stream
pub struct WhisperStream {
    engine: Arc<WhisperEngine>,
    /// Audio buffer for accumulating samples
    buffer: Arc<Mutex<Vec<f32>>>,
    /// Minimum audio duration to trigger transcription (seconds)
    min_duration_secs: f64,
    /// Maximum audio duration before forced transcription
    max_duration_secs: f64,
}

impl WhisperStream {
    /// Create a new Whisper stream
    pub fn new(engine: WhisperEngine) -> Result<Self> {
        Ok(Self {
            engine: Arc::new(engine),
            buffer: Arc::new(Mutex::new(Vec::new())),
            min_duration_secs: 1.0,  // 1 second minimum
            max_duration_secs: 30.0, // 30 second maximum
        })
    }

    /// Set minimum duration before transcribing
    pub fn with_min_duration(mut self, secs: f64) -> Self {
        self.min_duration_secs = secs;
        self
    }

    /// Set maximum duration before forced transcription
    pub fn with_max_duration(mut self, secs: f64) -> Self {
        self.max_duration_secs = secs;
        self
    }

    /// Add audio samples to the stream
    ///
    /// Returns transcription if enough audio accumulated
    pub fn feed(&self, samples: &[f32]) -> Result<Option<TranscriptionResult>> {
        let mut buffer = self.buffer.lock().unwrap();
        buffer.extend_from_slice(samples);

        let buffer_duration = buffer.len() as f64 / WHISPER_SAMPLE_RATE as f64;

        // Check if we should transcribe
        if buffer_duration >= self.min_duration_secs {
            // Clone buffer and clear
            let audio = buffer.clone();
            buffer.clear();

            // Transcribe
            drop(buffer); // Release lock during transcription
            let result = self.engine.transcribe(&audio)?;

            Ok(Some(result))
        } else if buffer_duration >= self.max_duration_secs {
            // Force transcription at max duration
            let audio = buffer.clone();
            buffer.clear();

            drop(buffer);
            let result = self.engine.transcribe(&audio)?;

            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    /// Force transcription of accumulated audio
    pub fn flush(&self) -> Result<Option<TranscriptionResult>> {
        let mut buffer = self.buffer.lock().unwrap();

        if buffer.is_empty() {
            return Ok(None);
        }

        let audio = buffer.clone();
        buffer.clear();

        drop(buffer);
        let result = self.engine.transcribe(&audio)?;

        Ok(Some(result))
    }

    /// Clear accumulated audio without transcribing
    pub fn clear(&self) {
        self.buffer.lock().unwrap().clear();
    }

    /// Get current buffer duration in seconds
    pub fn buffer_duration_secs(&self) -> f64 {
        let buffer = self.buffer.lock().unwrap();
        buffer.len() as f64 / WHISPER_SAMPLE_RATE as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::vad::SpeechSegment;

    #[test]
    fn test_whisper_config_default() {
        let config = WhisperConfig::default();
        assert_eq!(config.model_size, "base");
        assert_eq!(config.threads, 4);
        assert!(config.language.is_none());
    }

    #[test]
    fn test_whisper_config_model_size() {
        let config = WhisperConfig::with_model_size("small");
        assert_eq!(config.model_size, "small");
    }

    #[test]
    fn test_model_filename() {
        let config = WhisperConfig::with_model_size("tiny");
        assert_eq!(config.model_filename(), "ggml-tiny.bin");
    }

    #[test]
    fn test_get_model_info() {
        assert!(get_model_info("base").is_some());
        assert!(get_model_info("invalid").is_none());
    }

    #[test]
    fn test_preprocess_audio_mono() {
        let samples = vec![0.5f32; 16000]; // 1 second at 16kHz
        let result = preprocess_audio(&samples, 16000, 1);
        assert_eq!(result.len(), 16000);
    }

    #[test]
    fn test_preprocess_audio_stereo() {
        let samples: Vec<f32> = (0..32000).map(|i| i as f32 / 32000.0).collect();
        let result = preprocess_audio(&samples, 16000, 2);
        assert_eq!(result.len(), 16000); // Converted to mono
    }

    #[test]
    fn test_preprocess_audio_resample() {
        let samples = vec![0.5f32; 32000]; // 1 second at 32kHz
        let result = preprocess_audio(&samples, 32000, 1);
        assert_eq!(result.len(), 16000); // Resampled to 16kHz
    }

    #[test]
    fn test_resample_linear() {
        let samples = vec![0.0f32, 0.5, 1.0, 0.5, 0.0];
        let resampled = resample_linear(&samples, 1000, 500);
        assert_eq!(resampled.len(), 2); // Half the samples
    }

    #[test]
    fn test_list_available_models() {
        // Just ensure it doesn't panic
        list_available_models();
    }

    #[test]
    fn test_whisper_config_from_env() {
        // This test just validates the function doesn't panic
        // We can't set env vars in tests that would affect other tests
        let _config = WhisperConfig::from_env();
    }

    #[test]
    fn test_transcription_segment_duration() {
        let segment = TranscriptionSegment {
            text: "Hello".to_string(),
            start_secs: 0.0,
            end_secs: 1.5,
            probability: 0.95,
        };

        let speech = SpeechSegment {
            start_frame: 0,
            end_frame: 100,
            start_time_secs: 0.0,
            end_time_secs: 1.5,
            duration_secs: 1.5,
            avg_energy: 0.5,
            peak_energy: 0.8,
        };

        assert_eq!(speech.duration().as_secs_f64(), 1.5);
    }
}

/// Transcribe audio using faster-whisper Python package
///
/// This function calls faster-whisper via Python subprocess for efficient transcription.
/// The faster-whisper package uses CTranslate2 for optimized inference.
pub async fn transcribe_audio(audio_bytes: &[u8]) -> Result<String> {
    use tokio::process::Command;
    use std::time::Duration;

    // Encode audio as base64 for passing to Python
    let audio_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, audio_bytes);

    // Call faster-whisper Python script
    let output = tokio::time::timeout(
        Duration::from_secs(30), // 30 second timeout
        Command::new("/usr/bin/python3")
            .arg("/home/rapheal/.local/bin/faster-whisper-server.py")
            .arg(&audio_b64)
            .arg("small") // Use small model
            .output()
    ).await
    .map_err(|_| anyhow::anyhow!("Transcription timeout"))?
    .map_err(|e| anyhow::anyhow!("Failed to run faster-whisper: {}", e))?;

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "faster-whisper error: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Parse JSON response
    let result: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| anyhow::anyhow!("Failed to parse transcription result: {}", e))?;

    // Check for error
    if let Some(error) = result.get("error").and_then(|e| e.as_str()) {
        if !error.is_empty() {
            return Err(anyhow::anyhow!("Transcription error: {}", error));
        }
    }

    // Get transcribed text
    let text = result.get("text")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    Ok(text)
}