//! Cron Heartbeat - proactive task execution on a schedule
//!
//! Checks a HEARTBEAT.md checklist on a configurable interval within
//! active hours. Uses LLM to evaluate the checklist and take actions.

use anyhow::{Context, Result};
use chrono::Local;
use std::path::PathBuf;
use tokio::sync::broadcast;
use tracing::{info, warn, debug};

/// Heartbeat configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CronHeartbeatConfig {
    /// Minutes between heartbeat ticks
    #[serde(default = "default_interval")]
    pub interval_minutes: u64,
    /// Start of active window (HH:MM, 24h format)
    #[serde(default = "default_active_start")]
    pub active_start: String,
    /// End of active window (HH:MM, 24h format)
    #[serde(default = "default_active_end")]
    pub active_end: String,
    /// Max consecutive errors before suspending
    #[serde(default = "default_max_errors")]
    pub max_consecutive_errors: u32,
    /// Base backoff duration in seconds
    #[serde(default = "default_base_backoff")]
    pub base_backoff_secs: u64,
    /// Maximum backoff duration in seconds
    #[serde(default = "default_max_backoff")]
    pub max_backoff_secs: u64,
}

fn default_interval() -> u64 { 30 }
fn default_active_start() -> String { "08:00".to_string() }
fn default_active_end() -> String { "22:00".to_string() }
fn default_max_errors() -> u32 { 5 }
fn default_base_backoff() -> u64 { 60 }
fn default_max_backoff() -> u64 { 3600 }

impl Default for CronHeartbeatConfig {
    fn default() -> Self {
        Self {
            interval_minutes: default_interval(),
            active_start: default_active_start(),
            active_end: default_active_end(),
            max_consecutive_errors: default_max_errors(),
            base_backoff_secs: default_base_backoff(),
            max_backoff_secs: default_max_backoff(),
        }
    }
}

/// Result of a single heartbeat tick
#[derive(Debug, Clone)]
pub enum HeartbeatOutcome {
    /// Nothing to do
    Ok,
    /// An action was taken
    ActionTaken(String),
    /// User notification needed
    NotifyUser(String),
    /// Error during tick
    Error(String),
    /// Outside active window, skipped
    OutsideWindow,
}

impl std::fmt::Display for HeartbeatOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeartbeatOutcome::Ok => write!(f, "OK"),
            HeartbeatOutcome::ActionTaken(s) => write!(f, "Action: {}", s),
            HeartbeatOutcome::NotifyUser(s) => write!(f, "Notify: {}", s),
            HeartbeatOutcome::Error(s) => write!(f, "Error: {}", s),
            HeartbeatOutcome::OutsideWindow => write!(f, "Outside active window"),
        }
    }
}

/// The heartbeat runner
pub struct CronHeartbeat {
    config: CronHeartbeatConfig,
    consecutive_errors: u32,
    data_dir: PathBuf,
}

impl CronHeartbeat {
    /// Create a new heartbeat with config
    pub fn new(config: CronHeartbeatConfig) -> Result<Self> {
        let data_dir = crate::config::data_dir()?;
        std::fs::create_dir_all(&data_dir)
            .context("Failed to create data directory")?;
        Ok(Self {
            config,
            consecutive_errors: 0,
            data_dir,
        })
    }

    /// Check if current time is within the active window
    pub fn is_within_active_window(&self) -> bool {
        let now = Local::now();
        let current_time = now.format("%H:%M").to_string();

        current_time >= self.config.active_start && current_time <= self.config.active_end
    }

    /// Calculate backoff duration based on error count
    pub fn backoff_duration(&self, errors: u32) -> std::time::Duration {
        let secs = self.config.base_backoff_secs * 2u64.saturating_pow(errors);
        let capped = secs.min(self.config.max_backoff_secs);
        std::time::Duration::from_secs(capped)
    }

    /// Load the HEARTBEAT.md checklist, creating a default if missing
    pub fn load_checklist(&self) -> Result<String> {
        let path = self.data_dir.join("HEARTBEAT.md");
        if path.exists() {
            Ok(std::fs::read_to_string(&path)?)
        } else {
            let default = "# Heartbeat Checklist\n\n\
                           ## Daily Tasks\n\n\
                           - [ ] Review any pending learning entries for promotion\n\
                           - [ ] Check for outdated dependencies if cargo-outdated is installed\n\
                           - [ ] Clean up temporary files older than 7 days\n\n\
                           ## Weekly Tasks\n\n\
                           - [ ] Review memory database size and clean up if needed\n\
                           - [ ] Check for new version of my-agent\n";
            std::fs::write(&path, default)?;
            Ok(default.to_string())
        }
    }

    /// Execute a single heartbeat tick
    pub async fn tick(&mut self) -> HeartbeatOutcome {
        // Check active window
        if !self.is_within_active_window() {
            return HeartbeatOutcome::OutsideWindow;
        }

        // Check error suspension
        if self.consecutive_errors >= self.config.max_consecutive_errors {
            return HeartbeatOutcome::Error(format!(
                "Suspended after {} consecutive errors", self.consecutive_errors
            ));
        }

        // Load checklist
        let checklist = match self.load_checklist() {
            Ok(c) => c,
            Err(e) => {
                self.consecutive_errors += 1;
                return HeartbeatOutcome::Error(format!("Failed to load checklist: {}", e));
            }
        };

        // Load today's daily log for context
        let daily_context = crate::memory::daily_log::DailyLogManager::new()
            .map(|m| m.load_today())
            .unwrap_or_default();

        // Build LLM prompt
        let prompt = format!(
            "You are a background agent performing a scheduled heartbeat check.\n\
             Current time: {}\n\n\
             ## Checklist\n\n{}\n\n\
             ## Today's Activity\n\n{}\n\n\
             Review the checklist items. For each:\n\
             - If it's already been done today (per the activity log), skip it.\n\
             - If it can be done now, describe the action briefly.\n\
             - If the user needs to be notified of something, mention it.\n\n\
             Respond with exactly one of:\n\
             HEARTBEAT_OK - nothing needs attention\n\
             ACTION_NEEDED: <brief description of what to do>\n\
             NOTIFY_USER: <message for the user>\n",
            Local::now().format("%Y-%m-%d %H:%M"),
            checklist,
            if daily_context.is_empty() { "No activity recorded yet today.".to_string() } else { daily_context },
        );

        // Call LLM
        let client = match crate::agent::llm::OpenRouterClient::from_keyring() {
            Ok(c) => c,
            Err(e) => {
                self.consecutive_errors += 1;
                return HeartbeatOutcome::Error(format!("Failed to create LLM client: {}", e));
            }
        };

        let model = crate::config::Config::load()
            .map(|c| c.models.utility.clone())
            .unwrap_or_else(|_| "z-ai/glm-5".to_string());

        let messages = vec![
            crate::agent::llm::ChatMessage::system("You are a background maintenance agent. Be concise."),
            crate::agent::llm::ChatMessage::user(prompt),
        ];

        match client.complete(&model, messages, Some(256)).await {
            Ok(response) => {
                self.consecutive_errors = 0;
                let response = response.trim();

                // Log to daily log
                if let Ok(log_mgr) = crate::memory::daily_log::DailyLogManager::new() {
                    let _ = log_mgr.append_entry(&crate::memory::daily_log::LogEntry {
                        content: format!("Heartbeat: {}", response),
                        entry_type: crate::memory::daily_log::LogEntryType::Custom("heartbeat".into()),
                    });
                }

                // Parse response
                if response.starts_with("HEARTBEAT_OK") {
                    HeartbeatOutcome::Ok
                } else if response.starts_with("NOTIFY_USER:") {
                    let msg = response.trim_start_matches("NOTIFY_USER:").trim();
                    HeartbeatOutcome::NotifyUser(msg.to_string())
                } else if response.starts_with("ACTION_NEEDED:") {
                    let action = response.trim_start_matches("ACTION_NEEDED:").trim();
                    HeartbeatOutcome::ActionTaken(action.to_string())
                } else {
                    // Unexpected format, treat as OK
                    debug!("Unexpected heartbeat response: {}", response);
                    HeartbeatOutcome::Ok
                }
            }
            Err(e) => {
                self.consecutive_errors += 1;
                HeartbeatOutcome::Error(format!("LLM call failed: {}", e))
            }
        }
    }

    /// Run the heartbeat loop until shutdown signal
    pub async fn run(&mut self, mut shutdown_rx: broadcast::Receiver<()>) {
        info!("Heartbeat started (interval: {}m, window: {}-{})",
            self.config.interval_minutes,
            self.config.active_start,
            self.config.active_end);

        loop {
            let sleep_duration = if self.consecutive_errors > 0 {
                self.backoff_duration(self.consecutive_errors)
            } else {
                std::time::Duration::from_secs(self.config.interval_minutes * 60)
            };

            tokio::select! {
                _ = tokio::time::sleep(sleep_duration) => {
                    let outcome = self.tick().await;
                    match &outcome {
                        HeartbeatOutcome::Ok => debug!("Heartbeat: OK"),
                        HeartbeatOutcome::OutsideWindow => debug!("Heartbeat: outside window"),
                        HeartbeatOutcome::ActionTaken(a) => info!("Heartbeat action: {}", a),
                        HeartbeatOutcome::NotifyUser(m) => info!("Heartbeat notification: {}", m),
                        HeartbeatOutcome::Error(e) => warn!("Heartbeat error: {}", e),
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("Heartbeat shutting down");
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heartbeat_config_defaults() {
        let config = CronHeartbeatConfig::default();
        assert_eq!(config.interval_minutes, 30);
        assert_eq!(config.active_start, "08:00");
        assert_eq!(config.active_end, "22:00");
    }

    #[test]
    fn test_backoff_duration() {
        let hb = CronHeartbeat {
            config: CronHeartbeatConfig::default(),
            consecutive_errors: 0,
            data_dir: PathBuf::from("/tmp"),
        };
        assert_eq!(hb.backoff_duration(0).as_secs(), 60);  // base
        assert_eq!(hb.backoff_duration(1).as_secs(), 120); // 60*2
        assert_eq!(hb.backoff_duration(2).as_secs(), 240); // 60*4
        assert_eq!(hb.backoff_duration(10).as_secs(), 3600); // capped at max
    }

    #[test]
    fn test_outcome_display() {
        assert_eq!(format!("{}", HeartbeatOutcome::Ok), "OK");
        assert_eq!(format!("{}", HeartbeatOutcome::OutsideWindow), "Outside active window");
    }
}
