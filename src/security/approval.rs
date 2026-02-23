//! Action approval system
//!
//! Requires user approval for dangerous operations with audit logging.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

/// Diff line types for preview
#[derive(Debug, Clone)]
pub enum DiffLine {
    Context(String),
    Removed(String),
    Added(String),
}

/// Diff hunk for preview
#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<DiffLine>,
}

/// Diff preview for file edits
#[derive(Debug, Clone)]
pub struct DiffPreview {
    pub original: String,
    pub modified: String,
    pub hunks: Vec<DiffHunk>,
}

/// Risk levels for operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskLevel::Low => write!(f, "LOW"),
            RiskLevel::Medium => write!(f, "MEDIUM"),
            RiskLevel::High => write!(f, "HIGH"),
            RiskLevel::Critical => write!(f, "CRITICAL"),
        }
    }
}

/// Types of actions that can be approved
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActionType {
    FileRead,
    FileWrite,
    FileDelete,
    CommandExecute,
    NetworkRequest,
    ApiCall,
    SystemModify,
    Custom(String),
}

impl ActionType {
    pub fn default_risk(&self) -> RiskLevel {
        match self {
            ActionType::FileRead => RiskLevel::Low,
            ActionType::FileWrite => RiskLevel::Medium,
            ActionType::FileDelete => RiskLevel::Critical,
            ActionType::CommandExecute => RiskLevel::High,
            ActionType::NetworkRequest => RiskLevel::Medium,
            ActionType::ApiCall => RiskLevel::Low,
            ActionType::SystemModify => RiskLevel::Critical,
            ActionType::Custom(_) => RiskLevel::High,
        }
    }
}

impl std::fmt::Display for ActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionType::FileRead => write!(f, "File Read"),
            ActionType::FileWrite => write!(f, "File Write"),
            ActionType::FileDelete => write!(f, "File Delete"),
            ActionType::CommandExecute => write!(f, "Command Execute"),
            ActionType::NetworkRequest => write!(f, "Network Request"),
            ActionType::ApiCall => write!(f, "API Call"),
            ActionType::SystemModify => write!(f, "System Modify"),
            ActionType::Custom(s) => write!(f, "{}", s),
        }
    }
}

/// An action that requires approval
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    /// Unique identifier
    pub id: String,
    /// Type of action
    pub action_type: ActionType,
    /// Description of the action
    pub description: String,
    /// Risk level
    pub risk_level: RiskLevel,
    /// Target of the action (file path, command, URL, etc.)
    pub target: String,
    /// Additional details
    pub details: HashMap<String, String>,
    /// When the action was requested
    pub requested_at: DateTime<Utc>,
}

/// Approval decision
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalDecision {
    Approved,
    Denied,
    ApprovedForSession,
}

/// Record of an approval
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRecord {
    /// The action that was requested
    pub action: Action,
    /// The decision made
    pub decision: ApprovalDecision,
    /// When the decision was made
    pub decided_at: DateTime<Utc>,
    /// Optional reason for the decision
    pub reason: Option<String>,
}

/// Session-based approvals (approved actions for current session)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionApproval {
    /// Action type pattern
    pub action_type: ActionType,
    /// Target pattern (can include wildcards)
    pub target_pattern: String,
    /// When it was approved
    pub approved_at: DateTime<Utc>,
    /// Expiration time
    pub expires_at: Option<DateTime<Utc>>,
}

/// Approval configuration
#[derive(Debug, Clone)]
pub struct ApprovalConfig {
    /// Minimum risk level that requires approval
    pub approval_threshold: RiskLevel,
    /// Auto-approve low-risk actions
    pub auto_approve_low_risk: bool,
    /// Session approval duration in minutes
    pub session_duration_minutes: i64,
    /// Enable audit logging
    pub enable_audit_log: bool,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            approval_threshold: RiskLevel::Medium,
            auto_approve_low_risk: true,
            session_duration_minutes: 60,
            enable_audit_log: true,
        }
    }
}

/// Action approval manager
#[derive(Clone)]
pub struct ApprovalManager {
    config: ApprovalConfig,
    /// Audit log of all approval requests
    audit_log: Arc<Mutex<Vec<ApprovalRecord>>>,
    /// Session-based approvals
    session_approvals: Arc<Mutex<Vec<SessionApproval>>>,
}

impl ApprovalManager {
    /// Create a new approval manager
    pub fn new(config: ApprovalConfig) -> Self {
        Self {
            config,
            audit_log: Arc::new(Mutex::new(Vec::new())),
            session_approvals: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(ApprovalConfig::default())
    }

    /// Check if an action needs approval
    pub fn needs_approval(&self, action: &Action) -> bool {
        // Check if there's a session approval
        if self.has_session_approval(action) {
            return false;
        }

        // Check risk level against threshold
        match (&action.risk_level, &self.config.approval_threshold) {
            (RiskLevel::Low, _) => !self.config.auto_approve_low_risk,
            (RiskLevel::Medium, RiskLevel::Low | RiskLevel::Medium) => true,
            (RiskLevel::High, RiskLevel::Low | RiskLevel::Medium | RiskLevel::High) => true,
            (RiskLevel::Critical, _) => true,
            _ => false,
        }
    }

    /// Check if there's a session approval for this action
    fn has_session_approval(&self, action: &Action) -> bool {
        let approvals = self.session_approvals.lock().unwrap();
        let now = Utc::now();

        approvals.iter().any(|approval| {
            // Check if expired
            if let Some(expires) = approval.expires_at {
                if now > expires {
                    return false;
                }
            }

            // Check action type match
            let type_matches = match (&approval.action_type, &action.action_type) {
                (ActionType::Custom(a), ActionType::Custom(b)) => a == b,
                (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b),
            };

            if !type_matches {
                return false;
            }

            // Check target pattern (simple wildcard matching)
            if approval.target_pattern == "*" {
                return true;
            }

            if approval.target_pattern.contains('*') {
                let parts: Vec<&str> = approval.target_pattern.split('*').collect();
                if parts.len() == 2 {
                    let prefix = parts[0];
                    let suffix = parts[1];
                    return action.target.starts_with(prefix) && action.target.ends_with(suffix);
                }
            }

            action.target == approval.target_pattern
        })
    }

    /// Request approval for an action
    pub fn request_approval(&self, action: Action) -> Result<ApprovalDecision> {
        // Log the request
        if self.config.enable_audit_log {
            self.log_request(&action);
        }

        // Check if already approved for session
        if self.has_session_approval(&action) {
            return Ok(ApprovalDecision::ApprovedForSession);
        }

        // Check if approval is needed
        if !self.needs_approval(&action) {
            self.record_decision(&action, ApprovalDecision::Approved, None);
            return Ok(ApprovalDecision::Approved);
        }

        // Prompt user for approval
        let decision = self.prompt_user(&action)?;

        // Record the decision
        self.record_decision(&action, decision, None);

        // If approved for session, add to session approvals
        if decision == ApprovalDecision::ApprovedForSession {
            self.add_session_approval(SessionApproval {
                action_type: action.action_type.clone(),
                target_pattern: action.target.clone(),
                approved_at: Utc::now(),
                expires_at: Some(Utc::now() + chrono::Duration::minutes(self.config.session_duration_minutes)),
            });
        }

        Ok(decision)
    }

    /// Request approval with diff preview for file edits
    pub fn request_approval_with_diff(
        &self,
        mut action: Action,
        original_content: &str,
        new_content: &str,
    ) -> Result<ApprovalDecision> {
        // Log the request
        if self.config.enable_audit_log {
            self.log_request(&action);
        }

        // Check if already approved for session
        if self.has_session_approval(&action) {
            return Ok(ApprovalDecision::ApprovedForSession);
        }

        // Check if approval is needed
        if !self.needs_approval(&action) {
            self.record_decision(&action, ApprovalDecision::Approved, None);
            return Ok(ApprovalDecision::Approved);
        }

        // Store diff info in action details
        let old_lines = original_content.lines().count();
        let new_lines = new_content.lines().count();
        action.details.insert("original_lines".to_string(), old_lines.to_string());
        action.details.insert("new_lines".to_string(), new_lines.to_string());

        // Display diff preview before prompting
        self.display_diff_preview(&action, original_content, new_content)?;

        // Prompt user for approval
        let decision = self.prompt_user_for_diff(&action)?;

        // Record the decision
        self.record_decision(&action, decision, None);

        // If approved for session, add to session approvals
        if decision == ApprovalDecision::ApprovedForSession {
            self.add_session_approval(SessionApproval {
                action_type: action.action_type.clone(),
                target_pattern: action.target.clone(),
                approved_at: Utc::now(),
                expires_at: Some(Utc::now() + chrono::Duration::minutes(self.config.session_duration_minutes)),
            });
        }

        Ok(decision)
    }

    /// Display diff preview for file edits
    fn display_diff_preview(&self, action: &Action, old: &str, new: &str) -> Result<()> {
        println!();
        println!("╔══════════════════════════════════════════════════════════════╗");
        println!("║  📝 FILE EDIT PREVIEW                                        ║");
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!("║  File: {:<53}║", truncate(&action.target, 53));
        println!("╚══════════════════════════════════════════════════════════════╝");
        println!();

        let old_lines: Vec<&str> = old.lines().collect();
        let new_lines: Vec<&str> = new.lines().collect();

        println!("\x1b[90m--- Original ({} lines)\x1b[0m", old_lines.len());
        println!("\x1b[90m+++ Modified ({} lines)\x1b[0m", new_lines.len());
        println!();

        // Show colored diff - simple line-by-line comparison
        let max_lines = old_lines.len().max(new_lines.len());
        let mut shown_lines = 0;
        let max_display_lines = 30; // Limit display to avoid overwhelming output

        for i in 0..max_lines {
            if shown_lines >= max_display_lines {
                let remaining = max_lines - shown_lines;
                println!("\x1b[90m... {} more lines ...\x1b[0m", remaining);
                break;
            }

            let old_line = old_lines.get(i);
            let new_line = new_lines.get(i);

            match (old_line, new_line) {
                (Some(o), Some(n)) if o != n => {
                    // Changed line
                    println!("\x1b[31m- {:4}: {}\x1b[0m", i + 1, o);
                    println!("\x1b[32m+ {:4}: {}\x1b[0m", i + 1, n);
                    shown_lines += 2;
                }
                (Some(o), None) => {
                    // Removed line
                    println!("\x1b[31m- {:4}: {}\x1b[0m", i + 1, o);
                    shown_lines += 1;
                }
                (None, Some(n)) => {
                    // Added line
                    println!("\x1b[32m+ {:4}: {}\x1b[0m", i + 1, n);
                    shown_lines += 1;
                }
                (Some(_), Some(_)) => {
                    // Context line (unchanged) - only show if near changes
                    // For simplicity, just show all context
                }
                _ => {}
            }
        }

        // Handle extra lines at the end
        if new_lines.len() > old_lines.len() {
            for (i, line) in new_lines[old_lines.len()..].iter().enumerate() {
                if shown_lines >= max_display_lines {
                    break;
                }
                println!("\x1b[32m+ {:4}: {}\x1b[0m", old_lines.len() + i + 1, line);
                shown_lines += 1;
            }
        }

        println!();
        Ok(())
    }

    /// Prompt the user for approval with diff-specific message
    fn prompt_user_for_diff(&self, action: &Action) -> Result<ApprovalDecision> {
        println!("\x1b[33mApprove this edit?\x1b[0m");
        println!();
        println!("Options:");
        println!("  [y] Yes, apply this edit");
        println!("  [n] No, discard this edit");
        println!("  [s] Approve all edits to this file this session");
        println!("  [a] Approve all file edits this session");
        println!();

        loop {
            print!("Your choice [y/n/s/a]: ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();

            match input.as_str() {
                "y" | "yes" => return Ok(ApprovalDecision::Approved),
                "n" | "no" => return Ok(ApprovalDecision::Denied),
                "s" | "session" => return Ok(ApprovalDecision::ApprovedForSession),
                "a" | "all" => {
                    // Approve all similar - add wildcard pattern
                    self.add_session_approval(SessionApproval {
                        action_type: action.action_type.clone(),
                        target_pattern: "*".to_string(),
                        approved_at: Utc::now(),
                        expires_at: Some(Utc::now() + chrono::Duration::minutes(self.config.session_duration_minutes)),
                    });
                    return Ok(ApprovalDecision::ApprovedForSession);
                }
                _ => {
                    println!("Invalid option. Please enter y, n, s, or a.");
                }
            }
        }
    }

    /// Prompt the user for approval
    fn prompt_user(&self, action: &Action) -> Result<ApprovalDecision> {
        println!();
        println!("╔══════════════════════════════════════════════════════════════╗");
        println!("║  ⚠️  APPROVAL REQUIRED                                        ║");
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!("║  Risk Level: {:<48}║", format!("{}", action.risk_level));
        println!("║  Action: {:<51}║", truncate(&format!("{}", action.action_type), 51));
        println!("║  Target: {:<51}║", truncate(&action.target, 51));
        println!("║  Details: {:<50}║", truncate(&action.description, 50));
        println!("╚══════════════════════════════════════════════════════════════╝");
        println!();
        println!("Options:");
        println!("  [y] Yes, approve this action");
        println!("  [n] No, deny this action");
        println!("  [s] Approve for this session ({} minutes)", self.config.session_duration_minutes);
        println!("  [a] Approve all similar actions this session");
        println!();

        loop {
            print!("Your choice [y/n/s/a]: ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();

            match input.as_str() {
                "y" | "yes" => return Ok(ApprovalDecision::Approved),
                "n" | "no" => return Ok(ApprovalDecision::Denied),
                "s" | "session" => return Ok(ApprovalDecision::ApprovedForSession),
                "a" | "all" => {
                    // Approve all similar - add wildcard pattern
                    self.add_session_approval(SessionApproval {
                        action_type: action.action_type.clone(),
                        target_pattern: "*".to_string(),
                        approved_at: Utc::now(),
                        expires_at: Some(Utc::now() + chrono::Duration::minutes(self.config.session_duration_minutes)),
                    });
                    return Ok(ApprovalDecision::ApprovedForSession);
                }
                _ => {
                    println!("Invalid option. Please enter y, n, s, or a.");
                }
            }
        }
    }

    /// Log an approval request
    fn log_request(&self, action: &Action) {
        tracing::info!(
            action_id = %action.id,
            action_type = ?action.action_type,
            risk_level = ?action.risk_level,
            target = %action.target,
            "Approval requested"
        );
    }

    /// Record an approval decision
    fn record_decision(&self, action: &Action, decision: ApprovalDecision, reason: Option<&str>) {
        let record = ApprovalRecord {
            action: action.clone(),
            decision,
            decided_at: Utc::now(),
            reason: reason.map(|s| s.to_string()),
        };

        let mut log = self.audit_log.lock().unwrap();
        log.push(record);

        tracing::info!(
            action_id = %action.id,
            decision = ?decision,
            "Approval decision recorded"
        );
    }

    /// Add a session approval
    pub fn add_session_approval(&self, approval: SessionApproval) {
        let mut approvals = self.session_approvals.lock().unwrap();
        approvals.push(approval);
    }

    /// Clear all session approvals
    pub fn clear_session_approvals(&self) {
        let mut approvals = self.session_approvals.lock().unwrap();
        approvals.clear();
    }

    /// Get the audit log
    pub fn get_audit_log(&self) -> Vec<ApprovalRecord> {
        self.audit_log.lock().unwrap().clone()
    }

    /// Create an action for file operations
    pub fn create_file_action(action_type: ActionType, path: &str, description: &str) -> Action {
        let risk = action_type.default_risk();
        Action {
            id: uuid::Uuid::new_v4().to_string(),
            action_type,
            description: description.to_string(),
            risk_level: risk,
            target: path.to_string(),
            details: HashMap::new(),
            requested_at: Utc::now(),
        }
    }

    /// Create an action for command execution
    pub fn create_command_action(command: &str, description: &str) -> Action {
        Action {
            id: uuid::Uuid::new_v4().to_string(),
            action_type: ActionType::CommandExecute,
            description: description.to_string(),
            risk_level: RiskLevel::High,
            target: command.to_string(),
            details: HashMap::new(),
            requested_at: Utc::now(),
        }
    }
}

impl Default for ApprovalManager {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Truncate a string to a maximum length
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_risk_level_comparison() {
        let manager = ApprovalManager::with_defaults();

        let low_action = Action {
            id: "test".to_string(),
            action_type: ActionType::FileRead,
            description: "Test".to_string(),
            risk_level: RiskLevel::Low,
            target: "/tmp/test.txt".to_string(),
            details: HashMap::new(),
            requested_at: Utc::now(),
        };

        // Low risk should not need approval with default config
        assert!(!manager.needs_approval(&low_action));
    }

    #[test]
    fn test_action_type_default_risk() {
        assert_eq!(ActionType::FileRead.default_risk(), RiskLevel::Low);
        assert_eq!(ActionType::FileWrite.default_risk(), RiskLevel::Medium);
        assert_eq!(ActionType::FileDelete.default_risk(), RiskLevel::Critical);
        assert_eq!(ActionType::CommandExecute.default_risk(), RiskLevel::High);
    }

    #[test]
    fn test_session_approval() {
        let manager = ApprovalManager::with_defaults();

        let action = Action {
            id: "test".to_string(),
            action_type: ActionType::FileWrite,
            description: "Test".to_string(),
            risk_level: RiskLevel::Medium,
            target: "/tmp/test.txt".to_string(),
            details: HashMap::new(),
            requested_at: Utc::now(),
        };

        // Initially needs approval
        assert!(manager.needs_approval(&action));

        // Add session approval
        manager.add_session_approval(SessionApproval {
            action_type: ActionType::FileWrite,
            target_pattern: "/tmp/test.txt".to_string(),
            approved_at: Utc::now(),
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
        });

        // Now should not need approval
        assert!(!manager.needs_approval(&action));
    }
}
