//! Learning Store - persistent storage for learnings, errors, and feature requests
//!
//! Writes structured entries to flat Markdown files under ~/.local/share/my-agent/learning/
//! Each entry has a unique ID, priority, status, and area classification.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::info;

/// Type of learning entry
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryType {
    Learning,
    Error,
    FeatureRequest,
}

impl std::fmt::Display for EntryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntryType::Learning => write!(f, "LRN"),
            EntryType::Error => write!(f, "ERR"),
            EntryType::FeatureRequest => write!(f, "FTR"),
        }
    }
}

/// Priority level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Priority::Low => write!(f, "low"),
            Priority::Medium => write!(f, "medium"),
            Priority::High => write!(f, "high"),
            Priority::Critical => write!(f, "critical"),
        }
    }
}

/// Entry lifecycle status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryStatus {
    New,
    Validated,
    Promoted,
    Resolved,
    Dismissed,
}

impl std::fmt::Display for EntryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntryStatus::New => write!(f, "new"),
            EntryStatus::Validated => write!(f, "validated"),
            EntryStatus::Promoted => write!(f, "promoted"),
            EntryStatus::Resolved => write!(f, "resolved"),
            EntryStatus::Dismissed => write!(f, "dismissed"),
        }
    }
}

/// A single learning entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningEntry {
    pub id: String,
    pub entry_type: EntryType,
    pub priority: Priority,
    pub status: EntryStatus,
    pub area: String,
    pub title: String,
    pub description: String,
    pub context: String,
    pub suggested_action: Option<String>,
    pub related_tools: Vec<String>,
    pub occurrences: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Persistent learning store backed by flat Markdown files
pub struct LearningStore {
    base_dir: PathBuf,
}

impl LearningStore {
    /// Create a new learning store at the default location
    pub fn new() -> Result<Self> {
        let base_dir = crate::config::data_dir()?.join("learning");
        std::fs::create_dir_all(&base_dir)
            .context("Failed to create learning directory")?;
        Ok(Self { base_dir })
    }

    /// Create with a custom base directory
    pub fn with_dir(base_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&base_dir)
            .context("Failed to create learning directory")?;
        Ok(Self { base_dir })
    }

    /// Generate a unique entry ID
    fn generate_id(&self, entry_type: &EntryType) -> String {
        let prefix = entry_type.to_string();
        let date = Utc::now().format("%Y%m%d");
        let counter = self.count_entries_today(entry_type).unwrap_or(0) + 1;
        format!("{}-{}-{:03}", prefix, date, counter)
    }

    /// Count entries created today for a given type
    fn count_entries_today(&self, entry_type: &EntryType) -> Result<u32> {
        let entries = self.load_entries_from_file(entry_type)?;
        let today = Utc::now().format("%Y%m%d").to_string();
        let count = entries.iter()
            .filter(|e| e.id.contains(&today))
            .count();
        Ok(count as u32)
    }

    /// Record a new learning
    pub fn record_learning(
        &self,
        area: &str,
        title: &str,
        description: &str,
        context: &str,
        suggested_action: Option<&str>,
        related_tools: Vec<String>,
        priority: Priority,
    ) -> Result<LearningEntry> {
        let entry = LearningEntry {
            id: self.generate_id(&EntryType::Learning),
            entry_type: EntryType::Learning,
            priority,
            status: EntryStatus::New,
            area: area.to_string(),
            title: title.to_string(),
            description: description.to_string(),
            context: context.to_string(),
            suggested_action: suggested_action.map(|s| s.to_string()),
            related_tools,
            occurrences: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        self.append_entry(&entry)?;
        info!("Recorded learning: {} - {}", entry.id, title);
        Ok(entry)
    }

    /// Record a new error
    pub fn record_error(
        &self,
        area: &str,
        title: &str,
        description: &str,
        context: &str,
        related_tools: Vec<String>,
        priority: Priority,
    ) -> Result<LearningEntry> {
        let entry = LearningEntry {
            id: self.generate_id(&EntryType::Error),
            entry_type: EntryType::Error,
            priority,
            status: EntryStatus::New,
            area: area.to_string(),
            title: title.to_string(),
            description: description.to_string(),
            context: context.to_string(),
            suggested_action: None,
            related_tools,
            occurrences: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        self.append_entry(&entry)?;
        info!("Recorded error: {} - {}", entry.id, title);
        Ok(entry)
    }

    /// Record a new feature request
    pub fn record_feature_request(
        &self,
        area: &str,
        title: &str,
        description: &str,
        context: &str,
        priority: Priority,
    ) -> Result<LearningEntry> {
        let entry = LearningEntry {
            id: self.generate_id(&EntryType::FeatureRequest),
            entry_type: EntryType::FeatureRequest,
            priority,
            status: EntryStatus::New,
            area: area.to_string(),
            title: title.to_string(),
            description: description.to_string(),
            context: context.to_string(),
            suggested_action: None,
            related_tools: vec![],
            occurrences: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        self.append_entry(&entry)?;
        info!("Recorded feature request: {} - {}", entry.id, title);
        Ok(entry)
    }

    /// Search entries by text query
    pub fn search(&self, query: &str) -> Result<Vec<LearningEntry>> {
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for entry_type in &[EntryType::Learning, EntryType::Error, EntryType::FeatureRequest] {
            let entries = self.load_entries_from_file(entry_type)?;
            for entry in entries {
                if entry.title.to_lowercase().contains(&query_lower)
                    || entry.description.to_lowercase().contains(&query_lower)
                    || entry.area.to_lowercase().contains(&query_lower)
                {
                    results.push(entry);
                }
            }
        }

        Ok(results)
    }

    /// Get entries by status
    pub fn get_by_status(&self, status: &EntryStatus) -> Result<Vec<LearningEntry>> {
        let mut results = Vec::new();
        for entry_type in &[EntryType::Learning, EntryType::Error, EntryType::FeatureRequest] {
            let entries = self.load_entries_from_file(entry_type)?;
            for entry in entries {
                if entry.status == *status {
                    results.push(entry);
                }
            }
        }
        Ok(results)
    }

    /// Get a specific entry by ID
    pub fn get_by_id(&self, id: &str) -> Result<Option<LearningEntry>> {
        let entry_type = if id.starts_with("LRN") {
            EntryType::Learning
        } else if id.starts_with("ERR") {
            EntryType::Error
        } else if id.starts_with("FTR") {
            EntryType::FeatureRequest
        } else {
            return Ok(None);
        };

        let entries = self.load_entries_from_file(&entry_type)?;
        Ok(entries.into_iter().find(|e| e.id == id))
    }

    /// Update an entry's status
    pub fn update_status(&self, id: &str, new_status: EntryStatus) -> Result<()> {
        let entry_type = if id.starts_with("LRN") {
            EntryType::Learning
        } else if id.starts_with("ERR") {
            EntryType::Error
        } else if id.starts_with("FTR") {
            EntryType::FeatureRequest
        } else {
            anyhow::bail!("Invalid entry ID format: {}", id);
        };

        let mut entries = self.load_entries_from_file(&entry_type)?;
        if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
            entry.status = new_status;
            entry.updated_at = Utc::now();
        }
        self.write_entries_file(&entry_type, &entries)?;
        Ok(())
    }

    /// Increment occurrence count for an entry
    pub fn increment_occurrences(&self, id: &str) -> Result<()> {
        let entry_type = if id.starts_with("LRN") {
            EntryType::Learning
        } else if id.starts_with("ERR") {
            EntryType::Error
        } else if id.starts_with("FTR") {
            EntryType::FeatureRequest
        } else {
            anyhow::bail!("Invalid entry ID format: {}", id);
        };

        let mut entries = self.load_entries_from_file(&entry_type)?;
        if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
            entry.occurrences += 1;
            entry.updated_at = Utc::now();
            // Auto-validate after 2 occurrences
            if entry.occurrences >= 2 && entry.status == EntryStatus::New {
                entry.status = EntryStatus::Validated;
                info!("Auto-validated entry {} after {} occurrences", id, entry.occurrences);
            }
        }
        self.write_entries_file(&entry_type, &entries)?;
        Ok(())
    }

    /// Promote an entry (mark as promoted)
    pub fn promote(&self, id: &str) -> Result<()> {
        self.update_status(id, EntryStatus::Promoted)
    }

    /// Dismiss an entry
    pub fn dismiss(&self, id: &str) -> Result<()> {
        self.update_status(id, EntryStatus::Dismissed)
    }

    /// Get all entries of a specific type
    pub fn get_all(&self, entry_type: &EntryType) -> Result<Vec<LearningEntry>> {
        self.load_entries_from_file(entry_type)
    }

    /// Find a similar existing entry (for deduplication)
    pub fn find_similar(&self, entry_type: &EntryType, title: &str, area: &str) -> Result<Option<LearningEntry>> {
        let entries = self.load_entries_from_file(entry_type)?;
        let title_lower = title.to_lowercase();
        Ok(entries.into_iter().find(|e| {
            e.area == area && (
                e.title.to_lowercase() == title_lower
                || levenshtein_similar(&e.title.to_lowercase(), &title_lower)
            )
        }))
    }

    // --- File I/O ---

    fn file_path(&self, entry_type: &EntryType) -> PathBuf {
        let filename = match entry_type {
            EntryType::Learning => "LEARNINGS.md",
            EntryType::Error => "ERRORS.md",
            EntryType::FeatureRequest => "FEATURE_REQUESTS.md",
        };
        self.base_dir.join(filename)
    }

    fn append_entry(&self, entry: &LearningEntry) -> Result<()> {
        let mut entries = self.load_entries_from_file(&entry.entry_type).unwrap_or_default();
        entries.push(entry.clone());
        self.write_entries_file(&entry.entry_type, &entries)
    }

    fn write_entries_file(&self, entry_type: &EntryType, entries: &[LearningEntry]) -> Result<()> {
        let path = self.file_path(entry_type);
        let header = match entry_type {
            EntryType::Learning => "# Learnings\n\nCaptures insights and patterns from interactions.\n",
            EntryType::Error => "# Errors\n\nCaptures errors and failures for analysis.\n",
            EntryType::FeatureRequest => "# Feature Requests\n\nCaptures missing capabilities and improvement ideas.\n",
        };

        let mut content = String::with_capacity(4096);
        content.push_str(header);
        content.push('\n');

        for entry in entries {
            content.push_str(&format!("## {} — {}\n\n", entry.id, entry.title));
            content.push_str(&format!("- **Priority**: {}\n", entry.priority));
            content.push_str(&format!("- **Status**: {}\n", entry.status));
            content.push_str(&format!("- **Area**: {}\n", entry.area));
            content.push_str(&format!("- **Occurrences**: {}\n", entry.occurrences));
            content.push_str(&format!("- **Created**: {}\n", entry.created_at.format("%Y-%m-%d %H:%M UTC")));
            content.push_str(&format!("- **Updated**: {}\n", entry.updated_at.format("%Y-%m-%d %H:%M UTC")));
            if !entry.related_tools.is_empty() {
                content.push_str(&format!("- **Tools**: {}\n", entry.related_tools.join(", ")));
            }
            content.push('\n');
            content.push_str(&format!("**Description**: {}\n\n", entry.description));
            content.push_str(&format!("**Context**: {}\n\n", entry.context));
            if let Some(action) = &entry.suggested_action {
                content.push_str(&format!("**Suggested Action**: {}\n\n", action));
            }
            content.push_str("---\n\n");
        }

        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }

    fn load_entries_from_file(&self, entry_type: &EntryType) -> Result<Vec<LearningEntry>> {
        let path = self.file_path(entry_type);
        if !path.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        // Also try loading from JSON sidecar for reliable round-tripping
        let json_path = path.with_extension("json");
        if json_path.exists() {
            let json_content = std::fs::read_to_string(&json_path)?;
            if let Ok(entries) = serde_json::from_str::<Vec<LearningEntry>>(&json_content) {
                return Ok(entries);
            }
        }

        // Parse from markdown as fallback
        self.parse_markdown_entries(&content, entry_type)
    }

    fn parse_markdown_entries(&self, content: &str, entry_type: &EntryType) -> Result<Vec<LearningEntry>> {
        let mut entries = Vec::new();
        let mut current_id = String::new();
        let mut current_title = String::new();
        let mut priority = Priority::Medium;
        let mut status = EntryStatus::New;
        let mut area = String::new();
        let mut occurrences: u32 = 1;
        let mut related_tools = Vec::new();
        let mut description = String::new();
        let mut context = String::new();
        let mut suggested_action: Option<String> = None;
        let mut created_at = Utc::now();
        let mut updated_at = Utc::now();
        let mut in_entry = false;

        for line in content.lines() {
            if line.starts_with("## ") && line.contains(" — ") {
                // Save previous entry if any
                if in_entry && !current_id.is_empty() {
                    entries.push(LearningEntry {
                        id: current_id.clone(),
                        entry_type: entry_type.clone(),
                        priority,
                        status: status.clone(),
                        area: area.clone(),
                        title: current_title.clone(),
                        description: description.trim().to_string(),
                        context: context.trim().to_string(),
                        suggested_action: suggested_action.clone(),
                        related_tools: related_tools.clone(),
                        occurrences,
                        created_at,
                        updated_at,
                    });
                }

                // Parse new entry header
                let parts: Vec<&str> = line.trim_start_matches("## ").splitn(2, " — ").collect();
                current_id = parts.first().unwrap_or(&"").to_string();
                current_title = parts.get(1).unwrap_or(&"").to_string();
                priority = Priority::Medium;
                status = EntryStatus::New;
                area = String::new();
                occurrences = 1;
                related_tools = Vec::new();
                description = String::new();
                context = String::new();
                suggested_action = None;
                created_at = Utc::now();
                updated_at = Utc::now();
                in_entry = true;
            } else if in_entry {
                if line.starts_with("- **Priority**: ") {
                    let val = line.trim_start_matches("- **Priority**: ");
                    priority = match val {
                        "low" => Priority::Low,
                        "high" => Priority::High,
                        "critical" => Priority::Critical,
                        _ => Priority::Medium,
                    };
                } else if line.starts_with("- **Status**: ") {
                    let val = line.trim_start_matches("- **Status**: ");
                    status = match val {
                        "validated" => EntryStatus::Validated,
                        "promoted" => EntryStatus::Promoted,
                        "resolved" => EntryStatus::Resolved,
                        "dismissed" => EntryStatus::Dismissed,
                        _ => EntryStatus::New,
                    };
                } else if line.starts_with("- **Area**: ") {
                    area = line.trim_start_matches("- **Area**: ").to_string();
                } else if line.starts_with("- **Occurrences**: ") {
                    occurrences = line.trim_start_matches("- **Occurrences**: ")
                        .parse().unwrap_or(1);
                } else if line.starts_with("- **Tools**: ") {
                    related_tools = line.trim_start_matches("- **Tools**: ")
                        .split(", ")
                        .map(|s| s.to_string())
                        .collect();
                } else if line.starts_with("- **Created**: ") {
                    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(
                        line.trim_start_matches("- **Created**: ").trim_end_matches(" UTC"),
                        "%Y-%m-%d %H:%M"
                    ) {
                        created_at = dt.and_utc();
                    }
                } else if line.starts_with("- **Updated**: ") {
                    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(
                        line.trim_start_matches("- **Updated**: ").trim_end_matches(" UTC"),
                        "%Y-%m-%d %H:%M"
                    ) {
                        updated_at = dt.and_utc();
                    }
                } else if line.starts_with("**Description**: ") {
                    description = line.trim_start_matches("**Description**: ").to_string();
                } else if line.starts_with("**Context**: ") {
                    context = line.trim_start_matches("**Context**: ").to_string();
                } else if line.starts_with("**Suggested Action**: ") {
                    suggested_action = Some(line.trim_start_matches("**Suggested Action**: ").to_string());
                }
            }
        }

        // Save last entry
        if in_entry && !current_id.is_empty() {
            entries.push(LearningEntry {
                id: current_id,
                entry_type: entry_type.clone(),
                priority,
                status,
                area,
                title: current_title,
                description: description.trim().to_string(),
                context: context.trim().to_string(),
                suggested_action,
                related_tools,
                occurrences,
                created_at,
                updated_at,
            });
        }

        // Write JSON sidecar for reliable round-tripping
        if !entries.is_empty() {
            let json_path = self.file_path(entry_type).with_extension("json");
            if let Ok(json) = serde_json::to_string_pretty(&entries) {
                let _ = std::fs::write(json_path, json);
            }
        }

        Ok(entries)
    }

    /// Get the base directory path
    pub fn base_dir(&self) -> &PathBuf {
        &self.base_dir
    }
}

/// Simple similarity check (contains or major overlap)
fn levenshtein_similar(a: &str, b: &str) -> bool {
    if a.len() < 5 || b.len() < 5 {
        return a == b;
    }
    // Check if one contains the other or they share >70% words
    if a.contains(b) || b.contains(a) {
        return true;
    }
    let a_words: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let b_words: std::collections::HashSet<&str> = b.split_whitespace().collect();
    let intersection = a_words.intersection(&b_words).count();
    let union = a_words.union(&b_words).count();
    if union == 0 {
        return false;
    }
    (intersection as f64 / union as f64) > 0.7
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_type_display() {
        assert_eq!(EntryType::Learning.to_string(), "LRN");
        assert_eq!(EntryType::Error.to_string(), "ERR");
        assert_eq!(EntryType::FeatureRequest.to_string(), "FTR");
    }

    #[test]
    fn test_priority_ordering() {
        assert!(Priority::Low < Priority::Medium);
        assert!(Priority::Medium < Priority::High);
        assert!(Priority::High < Priority::Critical);
    }

    #[test]
    fn test_levenshtein_similar() {
        assert!(levenshtein_similar("tool execution failed", "tool execution failed"));
        assert!(levenshtein_similar("tool execution failed for read_file", "tool execution failed"));
        assert!(!levenshtein_similar("hello world", "goodbye moon"));
    }
}
