//! My Agent - Personal AI Assistant Library
//!
//! A secure AI agent with:
//! - OpenRouter API integration for LLM calls
//! - Voice chat with Whisper, Piper TTS, and Silero VAD
//! - Soul/heartbeat engine for autonomous actions
//! - Dynamic skills system
//! - Security sandbox and approval system
//! - JWT authentication
//!
//! # Example
//!
//! ```ignore
//! use my_agent::agent::llm::OpenRouterClient;
//! use my_agent::config::Config;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let client = OpenRouterClient::from_keyring()?;
//!     let response = client.chat("Hello!").await?;
//!     println!("{}", response);
//!     Ok(())
//! }
//! ```

// Core modules (order matters for cross-module dependencies)
pub mod types;
pub mod memory;  // Must come before agent since agent depends on memory
pub mod agent;
pub mod config;
pub mod security;
pub mod server;
pub mod voice;
pub mod cli;

// Feature modules
pub mod soul;
pub mod skills;
pub mod doctor;
pub mod tools;
pub mod notifications;
pub mod orchestrator;
pub mod messaging;
pub mod metrics;  // Self-improvement metrics and learning
pub mod learning; // Self-improving learning system
pub mod hooks;    // Lifecycle hook system
pub mod gateway;  // Gateway daemon mode

// Re-export commonly used types for convenience
pub use agent::{
    llm::OpenRouterClient,
    conversation::Conversation,
};

pub use memory::{
    MemoryStore,
    MemoryConfig,
    ConversationRecord,
    KnowledgeEntry,
};

pub use config::Config;

pub use security::{
    set_api_key,
    get_api_key,
    delete_api_key,
    set_hf_api_key,
    get_hf_api_key,
    has_hf_api_key,
    FileSystemSandbox,
    ApprovalManager,
    PromptSanitizer,
};

pub use voice::{
    synthesis::SynthesisEngine,
};

pub use soul::{
    SoulEngine,
    start_heartbeat,
    stop_heartbeat,
    show_status,
};

pub use server::{
    ServerState,
    VoiceMode,
    start as start_server,
};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Library name
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Get the library info
pub fn info() -> String {
    format!("{} v{} - Personal AI Assistant Library", NAME, VERSION)
}
