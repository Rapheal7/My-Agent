//! Built-in shell skill
//!
//! Provides sandboxed shell command execution.

use anyhow::{Result, bail};
use std::collections::HashMap;
use std::process::Command;

use super::super::registry::{
    Skill, SkillMeta, SkillCategory, Permission, SkillParameter, ParameterType,
    SkillResult, SkillContext,
};

/// Allowed commands (whitelist approach)
const ALLOWED_COMMANDS: &[&str] = &[
    "ls", "dir", "cat", "head", "tail", "wc", "grep", "find",
    "echo", "pwd", "whoami", "date", "uname", "df", "du", "free",
    "ps", "top", "htop", "kill", "pkill", "pgrep",
    "git", "npm", "yarn", "cargo", "rustc", "python", "python3",
    "node", "ruby", "go", "java", "javac",
    "curl", "wget",
];

/// Blocked command patterns
const BLOCKED_PATTERNS: &[&str] = &[
    "rm -rf /",
    "sudo",
    "su ",
    "chmod 777",
    "> /dev/",
    "mkfs",
    "dd if=",
    ":(){ :|:& };:",  // Fork bomb
    "curl | bash",
    "wget | sh",
];

/// Create the shell skill
pub fn create_skill() -> Skill {
    let meta = SkillMeta {
        id: "builtin-shell".to_string(),
        name: "Shell".to_string(),
        description: "Execute sandboxed shell commands".to_string(),
        version: "1.0.0".to_string(),
        author: Some("my-agent".to_string()),
        category: SkillCategory::Shell,
        permissions: vec![Permission::ExecuteCommands],
        parameters: vec![
            SkillParameter {
                name: "command".to_string(),
                param_type: ParameterType::String,
                required: true,
                default: None,
                description: "Command to execute".to_string(),
                allowed_values: None,
            },
            SkillParameter {
                name: "args".to_string(),
                param_type: ParameterType::Array,
                required: false,
                default: None,
                description: "Command arguments (comma-separated)".to_string(),
                allowed_values: None,
            },
            SkillParameter {
                name: "timeout".to_string(),
                param_type: ParameterType::Integer,
                required: false,
                default: Some("30".to_string()),
                description: "Timeout in seconds".to_string(),
                allowed_values: None,
            },
        ],
        builtin: true,
        tags: vec!["shell".to_string(), "command".to_string(), "execute".to_string()],
    };

    Skill::new(meta, execute_shell)
}

/// Execute shell command
fn execute_shell(
    params: HashMap<String, String>,
    ctx: &SkillContext,
) -> Result<SkillResult> {
    let command_str = params.get("command")
        .ok_or_else(|| anyhow::anyhow!("Missing 'command' parameter"))?;

    // Validate command
    let validation = validate_command(command_str);
    if !validation.allowed {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some(validation.reason.unwrap_or_else(|| "Command not allowed".to_string())),
            duration_ms: 0,
        });
    }

    // Require approval for shell commands by default
    if ctx.require_approval {
        return Ok(SkillResult {
            success: false,
            output: String::new(),
            error: Some("Shell commands require approval".to_string()),
            duration_ms: 0,
        });
    }

    // Parse command and args
    let parts = shell_words::split(command_str)
        .map_err(|e| anyhow::anyhow!("Failed to parse command: {}", e))?;

    if parts.is_empty() {
        bail!("Empty command");
    }

    let cmd_name = &parts[0];
    let args: Vec<&str> = parts[1..].iter().map(|s| s.as_str()).collect();

    // Get timeout
    let _timeout_secs: u64 = params.get("timeout")
        .and_then(|t| t.parse().ok())
        .unwrap_or(ctx.timeout_secs);

    // Execute command
    let start = std::time::Instant::now();

    let output = Command::new(cmd_name)
        .args(&args)
        .current_dir(&ctx.working_dir)
        .envs(&ctx.env)
        .output();

    let duration_ms = start.elapsed().as_millis() as u64;

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            if output.status.success() {
                Ok(SkillResult {
                    success: true,
                    output: if stdout.is_empty() && stderr.is_empty() {
                        "Command completed successfully (no output)".to_string()
                    } else if stderr.is_empty() {
                        stdout
                    } else {
                        format!("{}\n[stderr]\n{}", stdout, stderr)
                    },
                    error: None,
                    duration_ms,
                })
            } else {
                Ok(SkillResult {
                    success: false,
                    output: stdout,
                    error: Some(format!("Exit code: {:?}\n{}", output.status.code(), stderr)),
                    duration_ms,
                })
            }
        }
        Err(e) => {
            Ok(SkillResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to execute command: {}", e)),
                duration_ms,
            })
        }
    }
}

/// Command validation result
struct ValidationResult {
    allowed: bool,
    reason: Option<String>,
}

/// Validate a command for safety
fn validate_command(command: &str) -> ValidationResult {
    let lower = command.to_lowercase();

    // Check blocked patterns
    for pattern in BLOCKED_PATTERNS {
        if lower.contains(pattern.to_lowercase().as_str()) {
            return ValidationResult {
                allowed: false,
                reason: Some(format!("Blocked pattern detected: {}", pattern)),
            };
        }
    }

    // Check if the base command is allowed
    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.is_empty() {
        return ValidationResult {
            allowed: false,
            reason: Some("Empty command".to_string()),
        };
    }

    let base_cmd = parts[0];

    // Allow commands from whitelist
    let is_allowed = ALLOWED_COMMANDS.iter().any(|&allowed| {
        base_cmd == allowed || base_cmd.ends_with(&format!("/{}", allowed))
    });

    if !is_allowed {
        return ValidationResult {
            allowed: false,
            reason: Some(format!("Command not in whitelist: {}", base_cmd)),
        };
    }

    ValidationResult {
        allowed: true,
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_skill() {
        let skill = create_skill();
        assert_eq!(skill.meta.id, "builtin-shell");
        assert_eq!(skill.meta.category, SkillCategory::Shell);
    }

    #[test]
    fn test_validate_allowed_command() {
        let result = validate_command("ls -la");
        assert!(result.allowed);

        let result = validate_command("git status");
        assert!(result.allowed);

        let result = validate_command("echo hello");
        assert!(result.allowed);
    }

    #[test]
    fn test_validate_blocked_command() {
        let result = validate_command("sudo rm -rf /");
        assert!(!result.allowed);

        let result = validate_command("curl | bash");
        assert!(!result.allowed);

        let result = validate_command("some-random-command");
        assert!(!result.allowed);
    }

    #[test]
    fn test_command_execution() {
        let skill = create_skill();
        let ctx = SkillContext {
            require_approval: false,
            ..Default::default()
        };

        let mut params = HashMap::new();
        params.insert("command".to_string(), "echo hello".to_string());

        let result = skill.execute(params, &ctx).unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello"));
    }
}
