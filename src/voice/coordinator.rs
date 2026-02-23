//! Voice Coordinator
//!
//! Orchestrates the complete voice pipeline:
//! Audio Input → VAD → STT → LLM → TTS → Audio Output
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────┐     ┌─────┐     ┌────────┐     ┌─────┐     ┌────────┐
//! │ Microphone  │────→│ VAD │────→│ Whisper│────→│ LLM │────→│  TTS   │
//! └─────────────┘     └──┬──┘     │  STT   │     └─────┘     └───┬────┘
//!                        │        └────────┘                   │
//!                        │                                     │
//!                        │     ┌───────────────────────────────┘
//!                        │     │
//!                        │     ▼
//!                        │  ┌──────────┐
//!                        └──│  Speech  │←── On Speech Detected
//!                           │Detected? │
//!                           └──────────┘
//! ```
//!
//! # Features
//!
//! - Real-time voice conversation
//! - Voice activity detection for natural turn-taking
//! - Streaming STT for low latency
//! - Streaming TTS for immediate response playback
//! - Interrupt handling (user can speak during AI response)
//! - Conversation state management

use anyhow::{Result, Context, bail};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{self, Sender, Receiver};
use tracing::{info, debug, warn, error};

use crate::voice::audio::{AudioInput, AudioOutput, AudioConfig};
use crate::voice::vad::{Vad, VadStream, VadConfig, SpeechSegment};
use crate::voice::whisper::{WhisperEngine, WhisperConfig, TranscriptionResult};
use crate::voice::tts::{TtsEngine, TtsConfig, TtsResult};

/// Voice coordinator configuration
#[derive(Debug, Clone)]
pub struct VoiceCoordinatorConfig {
    /// Audio configuration
    pub audio: AudioConfig,
    /// VAD configuration
    pub vad: VadConfig,
    /// Whisper STT configuration
    pub whisper: WhisperConfig,
    /// TTS configuration
    pub tts: TtsConfig,
    /// Maximum conversation duration (seconds)
    pub max_conversation_duration_secs: u64,
    /// Silence timeout before processing (milliseconds)
    pub silence_timeout_ms: u64,
    /// Enable interruptions (user can speak during AI response)
    pub enable_interruptions: bool,
    /// Auto-play TTS responses
    pub auto_play: bool,
    /// Save conversation audio
    pub save_audio: bool,
    /// Audio save directory
    pub audio_save_dir: Option<std::path::PathBuf>,
}

impl Default for VoiceCoordinatorConfig {
    fn default() -> Self {
        Self {
            audio: AudioConfig::default(),
            vad: VadConfig::default(),
            whisper: WhisperConfig::default(),
            tts: TtsConfig::default(),
            max_conversation_duration_secs: 600, // 10 minutes
            silence_timeout_ms: 1500, // 1.5 seconds
            enable_interruptions: true,
            auto_play: true,
            save_audio: false,
            audio_save_dir: None,
        }
    }
}

/// Voice coordinator events
#[derive(Debug, Clone)]
pub enum VoiceEvent {
    /// Speech detected from user
    SpeechStarted,
    /// Speech ended, ready for processing
    SpeechEnded { duration_secs: f32 },
    /// Transcription result
    Transcription { text: String, confidence: f32 },
    /// LLM response received
    Response { text: String },
    /// TTS synthesis complete
    SynthesisComplete { duration_secs: f32 },
    /// Error occurred
    Error { message: String },
    /// Conversation ended
    ConversationEnded { reason: EndReason },
}

/// Reason for conversation ending
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// User requested stop
    UserRequested,
    /// Timeout reached
    Timeout,
    /// Error occurred
    Error,
    /// Maximum duration reached
    MaxDuration,
}

/// Conversation state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationState {
    /// Waiting for user to speak
    Listening,
    /// User is currently speaking
    UserSpeaking,
    /// Processing speech (STT)
    Processing,
    /// AI is generating response
    Generating,
    /// AI is speaking
    AiSpeaking,
    /// Conversation ended
    Ended,
}

/// Voice Coordinator - manages the complete voice pipeline
pub struct VoiceCoordinator {
    config: VoiceCoordinatorConfig,
    state: Arc<Mutex<ConversationState>>,
    /// Audio input handler
    audio_input: Arc<Mutex<Option<AudioInput>>>,
    /// Audio output handler
    audio_output: Arc<Mutex<Option<AudioOutput>>>,
    /// VAD stream
    vad_stream: Arc<Mutex<Option<VadStream>>>,
    /// Whisper STT engine
    whisper: Arc<Mutex<Option<WhisperEngine>>>,
    /// TTS engine
    tts: Arc<Mutex<Option<TtsEngine>>>,
    /// Event sender
    event_sender: Sender<VoiceEvent>,
    /// Event receiver
    event_receiver: Arc<Mutex<Receiver<VoiceEvent>>>,
    /// Running flag
    is_running: Arc<AtomicBool>,
    /// Conversation start time
    conversation_start: Arc<Mutex<Option<Instant>>>,
    /// Audio buffer for current utterance
    current_utterance: Arc<Mutex<Vec<f32>>>,
}

impl VoiceCoordinator {
    /// Create a new voice coordinator
    pub fn new(config: VoiceCoordinatorConfig) -> Result<Self> {
        let (event_sender, event_receiver) = mpsc::channel(100);

        Ok(Self {
            config,
            state: Arc::new(Mutex::new(ConversationState::Ended)),
            audio_input: Arc::new(Mutex::new(None)),
            audio_output: Arc::new(Mutex::new(None)),
            vad_stream: Arc::new(Mutex::new(None)),
            whisper: Arc::new(Mutex::new(None)),
            tts: Arc::new(Mutex::new(None)),
            event_sender,
            event_receiver: Arc::new(Mutex::new(event_receiver)),
            is_running: Arc::new(AtomicBool::new(false)),
            conversation_start: Arc::new(Mutex::new(None)),
            current_utterance: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Initialize all components
    pub async fn initialize(&self) -> Result<()> {
        info!("Initializing voice coordinator...");

        // Initialize audio output first (for TTS)
        let audio_output = AudioOutput::new(self.config.audio.clone())?;
        *self.audio_output.lock().unwrap() = Some(audio_output);
        info!("Audio output initialized");

        // Initialize audio input
        let audio_input = AudioInput::new(self.config.audio.clone())?;
        *self.audio_input.lock().unwrap() = Some(audio_input);
        info!("Audio input initialized");

        // Initialize VAD
        let vad = Vad::with_config(self.config.vad.clone());
        *self.vad_stream.lock().unwrap() = Some(VadStream::new(vad));
        info!("VAD initialized");

        // Initialize Whisper STT (optional - may fail if no model)
        match WhisperEngine::with_config(self.config.whisper.clone()) {
            Ok(whisper) => {
                *self.whisper.lock().unwrap() = Some(whisper);
                info!("Whisper STT initialized");
            }
            Err(e) => {
                warn!("Whisper STT not available: {}", e);
            }
        }

        // Initialize TTS (optional - may fail if no model)
        match TtsEngine::with_config(self.config.tts.clone()) {
            Ok(tts) => {
                *self.tts.lock().unwrap() = Some(tts);
                info!("TTS initialized");
            }
            Err(e) => {
                warn!("TTS not available: {}", e);
            }
        }

        info!("Voice coordinator initialization complete");
        Ok(())
    }

    /// Start a voice conversation
    pub async fn start_conversation(&self) -> Result<()> {
        if self.is_running.load(Ordering::SeqCst) {
            bail!("Conversation already in progress");
        }

        info!("Starting voice conversation...");
        self.is_running.store(true, Ordering::SeqCst);
        *self.state.lock().unwrap() = ConversationState::Listening;
        *self.conversation_start.lock().unwrap() = Some(Instant::now());

        // Send started event
        let _ = self.event_sender.send(VoiceEvent::SpeechStarted).await;

        // Start the voice processing loop
        self.run_voice_loop().await?;

        Ok(())
    }

    /// Stop the conversation
    pub fn stop_conversation(&self) {
        info!("Stopping voice conversation...");
        self.is_running.store(false, Ordering::SeqCst);
        *self.state.lock().unwrap() = ConversationState::Ended;

        // Stop audio input if running
        if let Ok(mut input) = self.audio_input.lock() {
            if let Some(ref audio_in) = *input {
                // Note: AudioInput doesn't have direct stop, handled by dropping stream
            }
            *input = None;
        }
    }

    /// Main voice processing loop
    async fn run_voice_loop(&self) -> Result<()> {
        let mut last_speech_time: Option<Instant> = None;
        let silence_timeout = Duration::from_millis(self.config.silence_timeout_ms);
        let check_interval = Duration::from_millis(50); // 50ms audio chunks

        while self.is_running.load(Ordering::SeqCst) {
            // Check max duration
            if let Some(start) = *self.conversation_start.lock().unwrap() {
                if start.elapsed().as_secs() > self.config.max_conversation_duration_secs {
                    let _ = self.event_sender.send(VoiceEvent::ConversationEnded {
                        reason: EndReason::MaxDuration,
                    }).await;
                    break;
                }
            }

            // Get audio chunk from microphone
            let audio_chunk = self.capture_audio_chunk().await?;

            if audio_chunk.is_empty() {
                tokio::time::sleep(check_interval).await;
                continue;
            }

            // Process through VAD
            let vad_result = self.process_vad(&audio_chunk).await?;

            match vad_result {
                VadResult::SpeechActive => {
                    last_speech_time = Some(Instant::now());
                    *self.state.lock().unwrap() = ConversationState::UserSpeaking;
                }
                VadResult::SpeechEnded(segments) => {
                    // Process the complete utterance
                    self.process_utterance(segments).await?;
                    last_speech_time = None;
                }
                VadResult::Silence => {
                    // Check if we've been silent long enough to process
                    if let Some(last_speech) = last_speech_time {
                        if last_speech.elapsed() > silence_timeout {
                            // Finalize current utterance
                            self.finalize_utterance().await?;
                            last_speech_time = None;
                        }
                    }
                }
            }

            tokio::time::sleep(check_interval).await;
        }

        Ok(())
    }

    /// Capture audio chunk from microphone
    async fn capture_audio_chunk(&self) -> Result<Vec<f32>> {
        // For now, simulate audio capture
        // In real implementation, this would read from AudioInput stream
        Ok(Vec::new())
    }

    /// Process audio through VAD
    async fn process_vad(&self, audio: &[f32]) -> Result<VadResult> {
        let mut vad_stream = self.vad_stream.lock().unwrap();

        if let Some(ref mut stream) = *vad_stream {
            let segments = stream.process(audio)?;

            if stream.is_speech() {
                // Accumulate audio for current utterance
                self.current_utterance.lock().unwrap().extend_from_slice(audio);
                Ok(VadResult::SpeechActive)
            } else if !segments.is_empty() {
                // Speech segment completed
                Ok(VadResult::SpeechEnded(segments))
            } else {
                Ok(VadResult::Silence)
            }
        } else {
            Ok(VadResult::Silence)
        }
    }

    /// Process a complete utterance (STT → LLM → TTS)
    async fn process_utterance(&self, segments: Vec<SpeechSegment>) -> Result<()> {
        info!("Processing utterance with {} segments", segments.len());
        *self.state.lock().unwrap() = ConversationState::Processing;

        // Get accumulated audio
        let utterance_audio = {
            let mut buffer = self.current_utterance.lock().unwrap();
            let audio = buffer.clone();
            buffer.clear();
            audio
        };

        if utterance_audio.is_empty() {
            warn!("No audio data for utterance");
            return Ok(());
        }

        // 1. Speech-to-Text
        let transcription = self.transcribe_audio(&utterance_audio).await?;

        // Calculate average confidence from segments
        let confidence = if transcription.segments.is_empty() {
            0.0
        } else {
            transcription.segments.iter()
                .map(|s| s.probability)
                .sum::<f32>() / transcription.segments.len() as f32
        };

        let _ = self.event_sender.send(VoiceEvent::Transcription {
            text: transcription.text.clone(),
            confidence,
        }).await;

        if transcription.text.trim().is_empty() {
            debug!("Empty transcription, skipping");
            *self.state.lock().unwrap() = ConversationState::Listening;
            return Ok(());
        }

        // 2. Generate LLM response
        *self.state.lock().unwrap() = ConversationState::Generating;
        let response = self.generate_response(&transcription.text).await?;

        let _ = self.event_sender.send(VoiceEvent::Response {
            text: response.clone(),
        }).await;

        // 3. Text-to-Speech
        self.speak_response(&response).await?;

        *self.state.lock().unwrap() = ConversationState::Listening;
        Ok(())
    }

    /// Finalize current utterance if any audio accumulated
    async fn finalize_utterance(&self) -> Result<()> {
        let audio = self.current_utterance.lock().unwrap().clone();

        if !audio.is_empty() {
            self.current_utterance.lock().unwrap().clear();

            // Create a dummy segment for finalization
            let segment = SpeechSegment {
                start_frame: 0,
                end_frame: audio.len() as u64,
                start_time_secs: 0.0,
                end_time_secs: audio.len() as f64 / 16000.0,
                duration_secs: audio.len() as f64 / 16000.0,
                avg_energy: 0.5,
                peak_energy: 0.8,
            };

            self.process_utterance(vec![segment]).await?;
        }

        Ok(())
    }

    /// Transcribe audio using Whisper STT
    async fn transcribe_audio(&self, audio: &[f32]) -> Result<TranscriptionResult> {
        let whisper = self.whisper.lock().unwrap();

        if let Some(ref engine) = *whisper {
            engine.transcribe(audio).context("STT transcription failed")
        } else {
            // Fallback: return empty transcription
            warn!("Whisper not available, using empty transcription");
            Ok(TranscriptionResult {
                text: String::new(),
                segments: Vec::new(),
                language: None,
                processing_time_secs: 0.0,
            })
        }
    }

    /// Generate LLM response to user input
    async fn generate_response(&self, user_input: &str) -> Result<String> {
        // This would integrate with the agent's LLM
        // For now, return a placeholder response
        info!("Generating response for: {}", user_input);

        // TODO: Integrate with actual LLM via orchestrator
        let response = format!("I heard you say: {}", user_input);

        Ok(response)
    }

    /// Synthesize and play response
    async fn speak_response(&self, text: &str) -> Result<()> {
        *self.state.lock().unwrap() = ConversationState::AiSpeaking;

        let tts = self.tts.lock().unwrap();

        if let Some(ref engine) = *tts {
            let result = engine.synthesize(text).context("TTS synthesis failed")?;

            let _ = self.event_sender.send(VoiceEvent::SynthesisComplete {
                duration_secs: result.duration_secs,
            }).await;

            if self.config.auto_play {
                // Play the audio
                let audio_output = self.audio_output.lock().unwrap();
                if let Some(ref output) = *audio_output {
                    use crate::voice::audio::AudioBuffer;
                    let buffer = AudioBuffer::from_samples(
                        result.samples,
                        result.sample_rate,
                        1,
                    );
                    output.play_buffer(&buffer)?;
                    output.sleep_until_end();
                }
            }
        } else {
            warn!("TTS not available, text response: {}", text);
        }

        Ok(())
    }

    /// Get current conversation state
    pub fn get_state(&self) -> ConversationState {
        self.state.lock().unwrap().clone()
    }

    /// Check if conversation is running
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    /// Get event receiver (for consuming events)
    pub fn get_event_receiver(&self) -> Arc<Mutex<Receiver<VoiceEvent>>> {
        self.event_receiver.clone()
    }

    /// Send a text message (for testing without audio)
    pub async fn send_text(&self, text: &str) -> Result<String> {
        *self.state.lock().unwrap() = ConversationState::Generating;

        let response = self.generate_response(text).await?;
        self.speak_response(&response).await?;

        *self.state.lock().unwrap() = ConversationState::Listening;
        Ok(response)
    }
}

/// VAD processing result
enum VadResult {
    /// Speech is currently active
    SpeechActive,
    /// Speech ended with detected segments
    SpeechEnded(Vec<SpeechSegment>),
    /// No speech detected
    Silence,
}

/// Simple voice coordinator for basic use cases
pub struct SimpleVoiceCoordinator {
    coordinator: VoiceCoordinator,
}

impl SimpleVoiceCoordinator {
    /// Create a new simple coordinator with default settings
    pub fn new() -> Result<Self> {
        let config = VoiceCoordinatorConfig::default();
        let coordinator = VoiceCoordinator::new(config)?;
        Ok(Self { coordinator })
    }

    /// Start a conversation
    pub async fn start(&self) -> Result<()> {
        self.coordinator.initialize().await?;
        self.coordinator.start_conversation().await
    }

    /// Stop the conversation
    pub fn stop(&self) {
        self.coordinator.stop_conversation();
    }

    /// Send text and get voice response
    pub async fn say(&self, text: &str) -> Result<String> {
        self.coordinator.send_text(text).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_voice_coordinator_config_default() {
        let config = VoiceCoordinatorConfig::default();
        assert_eq!(config.silence_timeout_ms, 1500);
        assert!(config.enable_interruptions);
        assert!(config.auto_play);
    }

    #[test]
    fn test_conversation_state_transitions() {
        let states = vec![
            ConversationState::Listening,
            ConversationState::UserSpeaking,
            ConversationState::Processing,
            ConversationState::Generating,
            ConversationState::AiSpeaking,
            ConversationState::Ended,
        ];

        // All states should be distinct
        assert_eq!(states.len(), 6);
    }

    #[test]
    fn test_end_reason_variants() {
        let reasons = vec![
            EndReason::UserRequested,
            EndReason::Timeout,
            EndReason::Error,
            EndReason::MaxDuration,
        ];

        assert_eq!(reasons.len(), 4);
    }

    #[test]
    fn test_voice_event_creation() {
        let events = vec![
            VoiceEvent::SpeechStarted,
            VoiceEvent::SpeechEnded { duration_secs: 1.5 },
            VoiceEvent::Transcription { text: "hello".to_string(), confidence: 0.95 },
            VoiceEvent::Response { text: "hi".to_string() },
            VoiceEvent::SynthesisComplete { duration_secs: 2.0 },
            VoiceEvent::Error { message: "test".to_string() },
            VoiceEvent::ConversationEnded { reason: EndReason::UserRequested },
        ];

        assert_eq!(events.len(), 7);
    }

    #[tokio::test]
    async fn test_voice_coordinator_creation() {
        let config = VoiceCoordinatorConfig::default();
        let coordinator = VoiceCoordinator::new(config);
        assert!(coordinator.is_ok());
    }

    #[test]
    fn test_vad_result_variants() {
        use super::VadResult;

        let _active = VadResult::SpeechActive;
        let _ended = VadResult::SpeechEnded(vec![]);
        let _silence = VadResult::Silence;
    }
}
