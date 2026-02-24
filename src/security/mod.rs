//! Security module
//!
//! Provides security features for the agent:
//! - File system sandbox
//! - Action approval system
//! - Prompt injection defense
//! - Secrets management
//! - OS keyring integration

pub mod keyring;
pub mod secrets;
pub mod sandbox;
pub mod approval;
pub mod prompt;

use anyhow::Result;

// Re-export commonly used types
pub use sandbox::{FileSystemSandbox, SandboxConfig, SandboxResult, FileOperation, RiskLevel as SandboxRiskLevel};
pub use approval::{ApprovalManager, ApprovalConfig, ApprovalDecision, Action, ActionType, RiskLevel};
pub use prompt::{PromptSanitizer, InjectionCheckResult, InjectionRisk};
pub use secrets::{SecretsManager, SecretsConfig, Secret, SecretSource};

/// Set API key in secure keyring
pub fn set_api_key(key: &str) -> Result<()> {
    keyring::set_api_key(key)
}

/// Get API key from secure keyring
pub fn get_api_key() -> Result<String> {
    keyring::get_api_key()
}

/// Delete API key from keyring
pub fn delete_api_key() -> Result<()> {
    keyring::delete_api_key()
}

/// Set Hugging Face API key in secure keyring
pub fn set_hf_api_key(key: &str) -> Result<()> {
    keyring::set_hf_api_key(key)
}

/// Get Hugging Face API key from secure keyring
pub fn get_hf_api_key() -> Result<String> {
    keyring::get_hf_api_key()
}

/// Delete Hugging Face API key from keyring
pub fn delete_hf_api_key() -> Result<()> {
    keyring::delete_hf_api_key()
}

/// Check if Hugging Face API key is set
pub fn has_hf_api_key() -> bool {
    keyring::has_hf_api_key()
}

/// Set server password (hashes and stores securely)
pub fn set_server_password(password: &str) -> Result<()> {
    keyring::set_server_password(password)
}

/// Check if a server password has been configured
pub fn has_server_password() -> bool {
    keyring::has_server_password()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        // Just verify the types are accessible
        let _sandbox = FileSystemSandbox::new();
        let _approver = ApprovalManager::with_defaults();
        let _sanitizer = PromptSanitizer::new();
    }
}
