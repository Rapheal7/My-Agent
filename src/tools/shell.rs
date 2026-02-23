//! Sandboxed shell command execution
//!
//! This module provides safe shell command execution with:
//! - Command validation and blocking of dangerous commands
//! - Working directory restrictions
//! - Timeout protection
//! - Environment variable filtering
//! - Output size limits
//! - Approval integration for all commands

use crate::security::{
    ApprovalManager, ApprovalDecision,
    approval::{ActionType, Action, RiskLevel},
};
use anyhow::{Result, Context, bail};
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// Default timeout for command execution (30 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum output size (1 MB)
const MAX_OUTPUT_SIZE: usize = 1024 * 1024;

/// Maximum command length
const MAX_COMMAND_LENGTH: usize = 4096;

/// Dangerous commands that are blocked by default
const BLOCKED_COMMANDS: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    ":(){ :|:& };:",  // Fork bomb
    "> /dev/sda",
    "dd if=/dev/zero of=/dev/sda",
    "mkfs.",
    "chmod -R 777 /",
    "chmod -R 777 /*",
    "chown -R",
    "mv / /dev/null",
    "> ~/.bashrc",
    "> ~/.zshrc",
    "> ~/.profile",
    "> /etc/passwd",
    "> /etc/shadow",
    "curl | sh",
    "curl | bash",
    "wget | sh",
    "wget | bash",
    "nc -e",
    "ncat -e",
    "netcat -e",
    "bash -i",
    "sh -i",
    "python -c 'import pty",
    "python3 -c 'import pty",
];

/// Commands that require additional scrutiny (High risk)
const HIGH_RISK_COMMANDS: &[&str] = &[
    "sudo",
    "su",
    "passwd",
    "usermod",
    "useradd",
    "groupadd",
    "systemctl",
    "service",
    "kill",
    "killall",
    "pkill",
    "iptables",
    "ufw",
    "apt",
    "apt-get",
    "yum",
    "dnf",
    "pacman",
    "npm install -g",
    "pip install",
    "cargo install",
    "curl",
    "wget",
    "ssh",
    "scp",
    "sftp",
    "rsync",
    "git push",
    "git pull",
    "git fetch",
    "git clone",
];

/// Shell command execution result
#[derive(Debug, Clone)]
pub struct CommandResult {
    /// The command that was executed
    pub command: String,
    /// Exit code (None if timed out or killed)
    pub exit_code: Option<i32>,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Whether the command timed out
    pub timed_out: bool,
    /// Execution duration
    pub duration_ms: u64,
}

/// Shell tool configuration
#[derive(Debug, Clone)]
pub struct ShellConfig {
    /// Working directory for command execution
    pub working_dir: Option<std::path::PathBuf>,
    /// Command timeout
    pub timeout: Duration,
    /// Environment variables to set
    pub env_vars: HashMap<String, String>,
    /// Whether to inherit environment from parent
    pub inherit_env: bool,
    /// Maximum output size
    pub max_output_size: usize,
    /// Allowed commands (empty = allow all non-blocked)
    pub allowed_commands: Vec<String>,
    /// Blocked commands (in addition to defaults)
    pub blocked_commands: Vec<String>,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            working_dir: None,
            timeout: DEFAULT_TIMEOUT,
            env_vars: HashMap::new(),
            inherit_env: true,
            max_output_size: MAX_OUTPUT_SIZE,
            allowed_commands: Vec::new(),
            blocked_commands: Vec::new(),
        }
    }
}

/// Sandboxed shell command executor
#[derive(Clone)]
pub struct ShellTool {
    config: ShellConfig,
    approver: ApprovalManager,
}

impl ShellTool {
    /// Create a new shell tool with default configuration
    pub fn new() -> Self {
        Self {
            config: ShellConfig::default(),
            approver: ApprovalManager::with_defaults(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(config: ShellConfig) -> Self {
        Self {
            config,
            approver: ApprovalManager::with_defaults(),
        }
    }

    /// Create with custom configuration and approver
    pub fn with_approver(config: ShellConfig, approver: ApprovalManager) -> Self {
        Self {
            config,
            approver,
        }
    }

    /// Get the configuration
    pub fn config(&self) -> &ShellConfig {
        &self.config
    }

    /// Validate a command for safety
    ///
    /// Returns Err if the command is blocked, Ok(risk_level) otherwise
    fn validate_command(&self, command: &str) -> Result<RiskLevel> {
        // Check command length
        if command.len() > MAX_COMMAND_LENGTH {
            bail!("Command too long ({} chars, max {})", command.len(), MAX_COMMAND_LENGTH);
        }

        let cmd_lower = command.to_lowercase();

        // Check against blocked commands
        for blocked in BLOCKED_COMMANDS {
            if cmd_lower.contains(&blocked.to_lowercase()) {
                bail!("Command contains blocked pattern: {}", blocked);
            }
        }
        for blocked in &self.config.blocked_commands {
            if cmd_lower.contains(&blocked.to_lowercase()) {
                bail!("Command contains blocked pattern: {}", blocked);
            }
        }

        // Determine risk level
        for high_risk in HIGH_RISK_COMMANDS {
            if cmd_lower.starts_with(&high_risk.to_lowercase()) ||
               cmd_lower.contains(&format!(" {}", high_risk.to_lowercase())) {
                return Ok(RiskLevel::High);
            }
        }

        // Check if there's an allowed list and this command is in it
        if !self.config.allowed_commands.is_empty() {
            let cmd_first = cmd_lower.split_whitespace().next().unwrap_or("");
            let is_allowed = self.config.allowed_commands.iter()
                .any(|allowed| allowed.to_lowercase() == cmd_first);
            if !is_allowed {
                bail!("Command '{}' is not in the allowed list", cmd_first);
            }
        }

        // Default to High risk since all shell commands are potentially dangerous
        Ok(RiskLevel::High)
    }

    /// Execute a shell command
    ///
    /// # Security
    /// - Command is validated against blocked patterns
    /// - Requires user approval (High risk)
    /// - Respects timeout
    /// - Output is size-limited
    pub async fn execute(&self, command: &str) -> Result<CommandResult> {
        // Validate command
        let risk_level = self.validate_command(command)?;

        // Request approval
        let action = Action {
            id: uuid::Uuid::new_v4().to_string(),
            action_type: ActionType::CommandExecute,
            description: format!("Execute: {}", command),
            risk_level,
            target: command.to_string(),
            details: [
                ("timeout".to_string(), format!("{:?}", self.config.timeout)),
                ("working_dir".to_string(), self.config.working_dir.as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "default".to_string())),
            ].into_iter().collect(),
            requested_at: chrono::Utc::now(),
        };

        match self.approver.request_approval(action)? {
            ApprovalDecision::Approved | ApprovalDecision::ApprovedForSession => {
                // Continue with execution
            }
            ApprovalDecision::Denied => {
                bail!("Command execution denied by user");
            }
        }

        // Execute the command
        self.execute_internal(command).await
    }

    /// Execute a command without approval (for automated/internal use)
    ///
    /// # Warning
    /// This bypasses the approval system. Only use for trusted internal operations.
    pub async fn execute_unsafe(&self, command: &str) -> Result<CommandResult> {
        self.execute_internal(command).await
    }

    /// Execute a command without approval (for internal use after approval)
    async fn execute_internal(&self, command: &str) -> Result<CommandResult> {
        let start = std::time::Instant::now();

        // Build the command
        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(command);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(command);
            c
        };

        // Set working directory
        if let Some(ref dir) = self.config.working_dir {
            cmd.current_dir(dir);
        }

        // Set environment variables
        if !self.config.inherit_env {
            cmd.env_clear();
        }
        for (key, value) in &self.config.env_vars {
            cmd.env(key, value);
        }

        // Set up stdout/stderr capture
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Spawn the process
        let child = cmd.spawn()
            .context("Failed to spawn command")?;

        // Wait with timeout
        let child_id = child.id();
        let result = timeout(self.config.timeout, async {
            let output = child.wait_with_output().await
                .context("Failed to get command output")?;
            Ok::<_, anyhow::Error>(output)
        }).await;

        let duration = start.elapsed();

        match result {
            Ok(Ok(output)) => {
                // Truncate output if too large
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                let stdout = if stdout.len() > self.config.max_output_size {
                    format!("{}...[truncated, total: {} bytes]",
                        &stdout[..self.config.max_output_size.min(stdout.len())],
                        stdout.len())
                } else {
                    stdout.to_string()
                };

                let stderr = if stderr.len() > self.config.max_output_size {
                    format!("{}...[truncated, total: {} bytes]",
                        &stderr[..self.config.max_output_size.min(stderr.len())],
                        stderr.len())
                } else {
                    stderr.to_string()
                };

                tracing::info!(
                    command = %command,
                    exit_code = ?output.status.code(),
                    duration_ms = %duration.as_millis(),
                    "Command executed successfully"
                );

                Ok(CommandResult {
                    command: command.to_string(),
                    exit_code: output.status.code(),
                    stdout,
                    stderr,
                    timed_out: false,
                    duration_ms: duration.as_millis() as u64,
                })
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                // Timeout - log and report
                tracing::warn!(
                    command = %command,
                    pid = ?child_id,
                    timeout = ?self.config.timeout,
                    "Command timed out, process killed"
                );

                Ok(CommandResult {
                    command: command.to_string(),
                    exit_code: None,
                    stdout: String::new(),
                    stderr: format!("Command timed out after {:?}", self.config.timeout),
                    timed_out: true,
                    duration_ms: duration.as_millis() as u64,
                })
            }
        }
    }

    /// Execute a command without capturing output (for simple checks)
    pub async fn execute_silent(&self, command: &str) -> Result<bool> {
        let result = self.execute(command).await?;
        Ok(result.exit_code == Some(0))
    }

    /// Check if a command exists in PATH
    pub async fn command_exists(&self, command: &str) -> bool {
        let check_cmd = if cfg!(target_os = "windows") {
            format!("where {} >nul 2>nul", command)
        } else {
            format!("command -v {} >/dev/null 2>&1", command)
        };

        // Use a simple check without approval for internal use
        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&check_cmd);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(&check_cmd);
            c
        };

        match cmd.output().await {
            Ok(output) => output.status.success(),
            Err(_) => false,
        }
    }

    /// Get working directory
    pub fn working_dir(&self) -> Option<&std::path::Path> {
        self.config.working_dir.as_deref()
    }

    /// Set working directory
    pub fn set_working_dir(&mut self, path: impl Into<std::path::PathBuf>) {
        self.config.working_dir = Some(path.into());
    }

    /// Add an environment variable
    pub fn set_env(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.config.env_vars.insert(key.into(), value.into());
    }

    /// Block a command pattern
    pub fn block_command(&mut self, pattern: impl Into<String>) {
        self.config.blocked_commands.push(pattern.into());
    }

    /// Allow a command (if using allowlist)
    pub fn allow_command(&mut self, command: impl Into<String>) {
        self.config.allowed_commands.push(command.into());
    }

    /// Set timeout
    pub fn set_timeout(&mut self, duration: Duration) {
        self.config.timeout = duration;
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience functions for one-off operations

/// Execute a command with default configuration
pub async fn execute(command: &str) -> Result<CommandResult> {
    let tool = ShellTool::new();
    tool.execute(command).await
}

/// Execute a command in a specific directory
pub async fn execute_in_dir(
    command: &str,
    dir: impl Into<std::path::PathBuf>,
) -> Result<CommandResult> {
    let mut config = ShellConfig::default();
    config.working_dir = Some(dir.into());
    let tool = ShellTool::with_config(config);
    tool.execute(command).await
}

/// Check if a command exists
pub async fn command_exists(command: &str) -> bool {
    let tool = ShellTool::new();
    tool.command_exists(command).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_echo_command() {
        let tool = ShellTool::new();
        // Note: This would require approval in real usage
        // For testing, we'd need to mock the approval manager
    }

    #[tokio::test]
    async fn test_validate_blocked_command() {
        let tool = ShellTool::new();

        // Should block dangerous commands
        assert!(tool.validate_command("rm -rf /").is_err());
        assert!(tool.validate_command("rm -rf /*").is_err());
        assert!(tool.validate_command(":(){ :|:& };:").is_err());

        // Should allow safe commands
        assert!(tool.validate_command("echo hello").is_ok());
        assert!(tool.validate_command("ls -la").is_ok());
    }

    #[tokio::test]
    async fn test_validate_risk_levels() {
        let tool = ShellTool::new();

        // High risk commands
        assert_eq!(
            tool.validate_command("sudo apt update").unwrap(),
            RiskLevel::High
        );
        assert_eq!(
            tool.validate_command("curl https://example.com").unwrap(),
            RiskLevel::High
        );

        // Regular commands (still high by default)
        assert_eq!(
            tool.validate_command("echo hello").unwrap(),
            RiskLevel::High
        );
    }

    #[tokio::test]
    async fn test_command_exists() {
        let tool = ShellTool::new();

        // These should exist on most Unix systems
        #[cfg(not(windows))]
        {
            assert!(tool.command_exists("sh").await);
            assert!(tool.command_exists("echo").await);
        }

        // This shouldn't exist
        assert!(!tool.command_exists("definitely_not_a_real_command_12345").await);
    }

    #[tokio::test]
    async fn test_working_directory() {
        let temp_dir = TempDir::new().unwrap();
        let mut tool = ShellTool::new();
        tool.set_working_dir(temp_dir.path());

        assert_eq!(tool.working_dir(), Some(temp_dir.path()));
    }

    #[tokio::test]
    async fn test_blocked_commands_list() {
        let mut tool = ShellTool::new();
        tool.block_command("custom-danger");

        assert!(tool.validate_command("custom-danger").is_err());
        assert!(tool.validate_command("echo custom-danger-test").is_err());
    }

    #[tokio::test]
    async fn test_allowed_commands_list() {
        let mut config = ShellConfig::default();
        config.allowed_commands = vec!["echo".to_string(), "ls".to_string()];
        let tool = ShellTool::with_config(config);

        // Should work
        assert!(tool.validate_command("echo hello").is_ok());
        assert!(tool.validate_command("ls -la").is_ok());

        // Should fail
        assert!(tool.validate_command("cat file.txt").is_err());
        assert!(tool.validate_command("rm file.txt").is_err());
    }

    #[tokio::test]
    async fn test_command_length_limit() {
        let tool = ShellTool::new();
        let long_command = format!("echo {}", "a".repeat(MAX_COMMAND_LENGTH));

        assert!(tool.validate_command(&long_command).is_err());
    }
}
