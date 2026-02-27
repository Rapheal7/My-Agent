//! Daily Log System - append-only markdown logs with temporal context
//!
//! Creates dated markdown files (YYYY-MM-DD.md) under ~/.local/share/my-agent/memory/
//! Loads today + yesterday at session start for temporal awareness.

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};
use std::path::PathBuf;
use tracing::debug;

/// Types of log entries
#[derive(Debug, Clone)]
pub enum LogEntryType {
    ConversationSummary,
    Decision,
    Error,
    Learning,
    Custom(String),
}

impl std::fmt::Display for LogEntryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogEntryType::ConversationSummary => write!(f, "conversation"),
            LogEntryType::Decision => write!(f, "decision"),
            LogEntryType::Error => write!(f, "error"),
            LogEntryType::Learning => write!(f, "learning"),
            LogEntryType::Custom(s) => write!(f, "{}", s),
        }
    }
}

/// A single log entry
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub content: String,
    pub entry_type: LogEntryType,
}

/// Manages daily markdown log files
pub struct DailyLogManager {
    base_dir: PathBuf,
}

impl DailyLogManager {
    /// Create a new manager at the default data directory
    pub fn new() -> Result<Self> {
        let base_dir = crate::config::data_dir()?.join("memory");
        std::fs::create_dir_all(&base_dir)
            .context("Failed to create daily log directory")?;
        Ok(Self { base_dir })
    }

    /// Create with a custom directory
    pub fn with_dir(base_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&base_dir)
            .context("Failed to create daily log directory")?;
        Ok(Self { base_dir })
    }

    /// Get the path for a specific date's log
    fn log_path(&self, date: NaiveDate) -> PathBuf {
        self.base_dir.join(format!("{}.md", date.format("%Y-%m-%d")))
    }

    /// Append a timestamped entry to today's log
    pub fn append_entry(&self, entry: &LogEntry) -> Result<()> {
        let today = Local::now().date_naive();
        let path = self.log_path(today);

        let existing = if path.exists() {
            std::fs::read_to_string(&path)?
        } else {
            format!("# Daily Log - {}\n", today.format("%Y-%m-%d"))
        };

        let timestamp = Local::now().format("%H:%M:%S");
        let new_content = format!(
            "{}\n\n## [{}] {}\n\n{}\n",
            existing.trim_end(),
            timestamp,
            entry.entry_type,
            entry.content,
        );

        std::fs::write(&path, new_content)?;
        debug!("Appended {} entry to {}", entry.entry_type, path.display());
        Ok(())
    }

    /// Load today's log
    pub fn load_today(&self) -> String {
        let today = Local::now().date_naive();
        self.load_date(today)
    }

    /// Load yesterday's log
    pub fn load_yesterday(&self) -> String {
        let yesterday = Local::now().date_naive() - chrono::Duration::days(1);
        self.load_date(yesterday)
    }

    /// Load a specific date's log
    pub fn load_date(&self, date: NaiveDate) -> String {
        let path = self.log_path(date);
        if path.exists() {
            std::fs::read_to_string(&path).unwrap_or_default()
        } else {
            String::new()
        }
    }

    /// Load today + yesterday combined for session context
    pub fn load_recent_context(&self) -> String {
        let today = self.load_today();
        let yesterday = self.load_yesterday();

        let mut context = String::new();
        if !yesterday.is_empty() {
            context.push_str(&yesterday);
            context.push_str("\n\n---\n\n");
        }
        if !today.is_empty() {
            context.push_str(&today);
        }
        context
    }

    /// List all log file dates (sorted newest first)
    pub fn list_logs(&self) -> Result<Vec<NaiveDate>> {
        let mut dates = Vec::new();
        if !self.base_dir.exists() {
            return Ok(dates);
        }

        for entry in std::fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".md") {
                let date_str = name_str.trim_end_matches(".md");
                if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
                    dates.push(date);
                }
            }
        }

        dates.sort_by(|a, b| b.cmp(a)); // newest first
        Ok(dates)
    }

    /// Remove logs older than keep_days
    pub fn cleanup(&self, keep_days: u32) -> Result<usize> {
        let cutoff = Local::now().date_naive() - chrono::Duration::days(keep_days as i64);
        let mut removed = 0;

        for date in self.list_logs()? {
            if date < cutoff {
                let path = self.log_path(date);
                if path.exists() {
                    std::fs::remove_file(&path)?;
                    removed += 1;
                }
            }
        }

        Ok(removed)
    }

    /// Get the base directory
    pub fn base_dir(&self) -> &PathBuf {
        &self.base_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_entry_type_display() {
        assert_eq!(format!("{}", LogEntryType::ConversationSummary), "conversation");
        assert_eq!(format!("{}", LogEntryType::Decision), "decision");
        assert_eq!(format!("{}", LogEntryType::Error), "error");
        assert_eq!(format!("{}", LogEntryType::Learning), "learning");
        assert_eq!(format!("{}", LogEntryType::Custom("test".into())), "test");
    }

    #[test]
    fn test_daily_log_create_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = DailyLogManager::with_dir(dir.path().to_path_buf()).unwrap();

        // Should be empty initially
        assert!(mgr.load_today().is_empty());
        assert!(mgr.load_yesterday().is_empty());
        assert!(mgr.list_logs().unwrap().is_empty());

        // Append a decision
        mgr.append_entry(&LogEntry {
            content: "Decided to use Rhai for scripting".to_string(),
            entry_type: LogEntryType::Decision,
        }).unwrap();

        // Read it back
        let today = mgr.load_today();
        assert!(today.contains("Decided to use Rhai for scripting"), "Content missing from: {}", today);
        assert!(today.contains("decision"), "Entry type missing from: {}", today);
        assert!(today.contains("# Daily Log"), "Header missing from: {}", today);
    }

    #[test]
    fn test_daily_log_multiple_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = DailyLogManager::with_dir(dir.path().to_path_buf()).unwrap();

        // Append multiple different types
        mgr.append_entry(&LogEntry {
            content: "First entry".to_string(),
            entry_type: LogEntryType::ConversationSummary,
        }).unwrap();
        mgr.append_entry(&LogEntry {
            content: "Second entry - an error".to_string(),
            entry_type: LogEntryType::Error,
        }).unwrap();
        mgr.append_entry(&LogEntry {
            content: "Third entry - a learning".to_string(),
            entry_type: LogEntryType::Learning,
        }).unwrap();

        let today = mgr.load_today();
        assert!(today.contains("First entry"));
        assert!(today.contains("Second entry - an error"));
        assert!(today.contains("Third entry - a learning"));
        assert!(today.contains("conversation"));
        assert!(today.contains("error"));
        assert!(today.contains("learning"));

        // Still only one log file
        let logs = mgr.list_logs().unwrap();
        assert_eq!(logs.len(), 1);
    }

    #[test]
    fn test_daily_log_list_and_load_date() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = DailyLogManager::with_dir(dir.path().to_path_buf()).unwrap();

        // Manually create a log for a specific date
        let fake_date = NaiveDate::from_ymd_opt(2025, 6, 15).unwrap();
        let path = dir.path().join("2025-06-15.md");
        std::fs::write(&path, "# Daily Log - 2025-06-15\n\n## [10:00:00] decision\n\nOld decision\n").unwrap();

        // Also create today's
        mgr.append_entry(&LogEntry {
            content: "Today's entry".to_string(),
            entry_type: LogEntryType::Decision,
        }).unwrap();

        // Should list both
        let logs = mgr.list_logs().unwrap();
        assert_eq!(logs.len(), 2, "Expected 2 logs, got: {:?}", logs);

        // Load the fake date
        let old_content = mgr.load_date(fake_date);
        assert!(old_content.contains("Old decision"), "Old content not found: {}", old_content);

        // Load a nonexistent date
        let missing = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
        assert!(mgr.load_date(missing).is_empty());
    }

    #[test]
    fn test_daily_log_recent_context() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = DailyLogManager::with_dir(dir.path().to_path_buf()).unwrap();

        // Create today
        mgr.append_entry(&LogEntry {
            content: "Today's work".to_string(),
            entry_type: LogEntryType::ConversationSummary,
        }).unwrap();

        // Create yesterday manually
        let yesterday = Local::now().date_naive() - chrono::Duration::days(1);
        let yesterday_path = dir.path().join(format!("{}.md", yesterday.format("%Y-%m-%d")));
        std::fs::write(&yesterday_path, "# Yesterday\n\nYesterday's work was important.\n").unwrap();

        let context = mgr.load_recent_context();
        assert!(context.contains("Today's work"), "Today missing from context");
        assert!(context.contains("Yesterday's work was important"), "Yesterday missing from context");
        // Yesterday should come before today
        let yesterday_pos = context.find("Yesterday's work").unwrap();
        let today_pos = context.find("Today's work").unwrap();
        assert!(yesterday_pos < today_pos, "Yesterday should appear before today");
    }

    #[test]
    fn test_daily_log_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = DailyLogManager::with_dir(dir.path().to_path_buf()).unwrap();

        // Create several old log files
        for days_ago in [0, 5, 15, 30, 60] {
            let date = Local::now().date_naive() - chrono::Duration::days(days_ago);
            let path = dir.path().join(format!("{}.md", date.format("%Y-%m-%d")));
            std::fs::write(&path, format!("# Log for {} days ago", days_ago)).unwrap();
        }

        assert_eq!(mgr.list_logs().unwrap().len(), 5);

        // Cleanup: keep last 20 days (should remove 30 and 60 day old ones)
        let removed = mgr.cleanup(20).unwrap();
        assert_eq!(removed, 2, "Should have removed 2 old logs");
        assert_eq!(mgr.list_logs().unwrap().len(), 3);

        // The recent ones should still exist
        let today = mgr.load_today();
        assert!(!today.is_empty());
    }

    #[test]
    fn test_daily_log_file_format() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = DailyLogManager::with_dir(dir.path().to_path_buf()).unwrap();

        mgr.append_entry(&LogEntry {
            content: "User asked about Rust lifetimes".to_string(),
            entry_type: LogEntryType::ConversationSummary,
        }).unwrap();

        let today = mgr.load_today();
        // Verify markdown structure
        assert!(today.starts_with("# Daily Log - "), "Should start with header");
        assert!(today.contains("## ["), "Should have timestamped sections");
        assert!(today.contains("] conversation"), "Should have entry type after timestamp");
    }
}
