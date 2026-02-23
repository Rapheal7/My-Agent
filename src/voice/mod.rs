//! My Agent - Voice Module
//!
//! Voice processing modules using local implementations:
//! - Whisper (STT - Speech-to-Text)
//! - Piper TTS (Text-to-Speech)
//! - Silero VAD (Voice Activity Detection)
//! - Audio I/O (microphone input, speaker output)
//! - Multi-agent result synthesis
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use my_agent::voice::synthesis::{SynthesisEngine, AgentResult, helpers};
//!
//! # fn main() -> anyhow::Result<()> {
//! // Create synthesis engine
//! let engine = SynthesisEngine::new();
//!
//! // Add agent results
//! let results = vec![
//!     AgentResult::new("agent1", "code", "gpt-4", "Hello world"),
//! ];
//!
//! // Synthesize into voice response
//! let synthesized = engine.synthesize(results)?;
//! println!("{}", synthesized.text);
//! # Ok(())
//! # }
//! ```

pub mod synthesis;
pub mod coordinator;
pub mod integration;
pub mod audio;
pub mod vad;
pub mod whisper;
pub mod tts;

use anyhow::Result;

// Re-export synthesis types for convenience
pub use synthesis::{
    SynthesisEngine,
    SynthesisConfig,
    SynthesisStrategy,
    SynthesizedResult,
    AgentResult,
    VoiceResponseBuilder,
    helpers as synthesis_helpers,
};
