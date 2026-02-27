//! Learning Detector - automatic detection of learnings from interactions
//!
//! Scans user messages, assistant responses, and tool outcomes for patterns
//! that indicate a learning opportunity, error, or missing capability.

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, info};

use super::store::{LearningStore, EntryType, Priority, LearningEntry};

/// A detected learning event
#[derive(Debug, Clone)]
pub struct DetectedEvent {
    pub event_type: EntryType,
    pub area: String,
    pub title: String,
    pub description: String,
    pub context: String,
    pub priority: Priority,
    pub related_tools: Vec<String>,
    pub suggested_action: Option<String>,
}

/// Patterns that indicate user corrections
const CORRECTION_PATTERNS: &[&str] = &[
    "no, that's wrong",
    "actually,",
    "that's not what i meant",
    "not that,",
    "i meant",
    "no, i wanted",
    "wrong file",
    "wrong path",
    "that's incorrect",
    "you misunderstood",
    "try again",
    "that's not right",
    "please fix",
    "not what i asked",
];

/// Patterns that indicate missing capability
const CAPABILITY_PATTERNS: &[&str] = &[
    "i can't do that",
    "i don't have the ability",
    "that's not supported",
    "i'm unable to",
    "no tool available",
    "outside my capabilities",
    "i don't know how to",
];

/// Automatic learning detector
pub struct LearningDetector {
    store: Arc<LearningStore>,
}

impl LearningDetector {
    /// Create a new detector
    pub fn new(store: Arc<LearningStore>) -> Self {
        Self { store }
    }

    /// Detect learnings from a user-assistant exchange
    pub fn detect_from_response(
        &self,
        user_msg: &str,
        assistant_msg: &str,
    ) -> Vec<DetectedEvent> {
        let mut events = Vec::new();
        let user_lower = user_msg.to_lowercase();

        // Check for correction patterns in user message
        for pattern in CORRECTION_PATTERNS {
            if user_lower.contains(pattern) {
                events.push(DetectedEvent {
                    event_type: EntryType::Learning,
                    area: "user_correction".to_string(),
                    title: format!("User correction detected: {}", truncate(user_msg, 60)),
                    description: format!(
                        "User corrected the assistant. User said: '{}'. Previous response context: '{}'",
                        truncate(user_msg, 200),
                        truncate(assistant_msg, 200)
                    ),
                    context: format!("Correction pattern '{}' matched", pattern),
                    priority: Priority::Medium,
                    related_tools: vec![],
                    suggested_action: Some("Adjust response patterns to avoid this type of error".to_string()),
                });
                break; // Only one correction event per message
            }
        }

        events
    }

    /// Detect learnings from a tool failure
    pub fn detect_from_tool_failure(
        &self,
        tool_name: &str,
        error: &str,
    ) -> Option<DetectedEvent> {
        let priority = if error.contains("permission denied") || error.contains("not found") {
            Priority::Medium
        } else if error.contains("timeout") || error.contains("rate limit") {
            Priority::High
        } else {
            Priority::Low
        };

        // Classify the error area
        let area = classify_tool_area(tool_name);

        Some(DetectedEvent {
            event_type: EntryType::Error,
            area,
            title: format!("Tool '{}' failed", tool_name),
            description: format!("Tool '{}' execution failed with error: {}", tool_name, truncate(error, 300)),
            context: format!("During tool execution of '{}'", tool_name),
            priority,
            related_tools: vec![tool_name.to_string()],
            suggested_action: None,
        })
    }

    /// Detect efficient patterns from tool successes
    pub fn detect_from_tool_success(
        &self,
        tool_name: &str,
        duration_ms: u64,
        _pattern: &str,
    ) -> Option<DetectedEvent> {
        // Only capture notably fast or interesting executions
        if duration_ms > 5000 {
            return Some(DetectedEvent {
                event_type: EntryType::Learning,
                area: "performance".to_string(),
                title: format!("Slow tool execution: {} ({}ms)", tool_name, duration_ms),
                description: format!(
                    "Tool '{}' took {}ms to execute, which is above the 5s threshold",
                    tool_name, duration_ms
                ),
                context: "Performance monitoring".to_string(),
                priority: Priority::Low,
                related_tools: vec![tool_name.to_string()],
                suggested_action: Some("Consider optimizing or caching results".to_string()),
            });
        }
        None
    }

    /// Detect missing capabilities from assistant responses
    pub fn detect_missing_capability(
        &self,
        user_msg: &str,
        assistant_msg: &str,
    ) -> Option<DetectedEvent> {
        let assistant_lower = assistant_msg.to_lowercase();

        for pattern in CAPABILITY_PATTERNS {
            if assistant_lower.contains(pattern) {
                return Some(DetectedEvent {
                    event_type: EntryType::FeatureRequest,
                    area: "missing_capability".to_string(),
                    title: format!("Missing capability: {}", truncate(user_msg, 60)),
                    description: format!(
                        "User requested: '{}'. Assistant indicated inability: '{}'",
                        truncate(user_msg, 200),
                        truncate(assistant_msg, 200)
                    ),
                    context: format!("Capability pattern '{}' matched in response", pattern),
                    priority: Priority::Medium,
                    related_tools: vec![],
                    suggested_action: Some("Consider adding this capability via a new tool or skill".to_string()),
                });
            }
        }

        None
    }

    /// Process a detected event: deduplicate and store
    pub fn process_event(&self, event: DetectedEvent) -> Result<Option<LearningEntry>> {
        // Check for duplicates
        if let Some(existing) = self.store.find_similar(
            &event.event_type,
            &event.title,
            &event.area,
        )? {
            // Increment occurrences instead of creating duplicate
            self.store.increment_occurrences(&existing.id)?;
            debug!("Incremented occurrence for existing entry: {}", existing.id);
            return Ok(None);
        }

        // Record new entry
        let entry = match event.event_type {
            EntryType::Learning => self.store.record_learning(
                &event.area,
                &event.title,
                &event.description,
                &event.context,
                event.suggested_action.as_deref(),
                event.related_tools,
                event.priority,
            )?,
            EntryType::Error => self.store.record_error(
                &event.area,
                &event.title,
                &event.description,
                &event.context,
                event.related_tools,
                event.priority,
            )?,
            EntryType::FeatureRequest => self.store.record_feature_request(
                &event.area,
                &event.title,
                &event.description,
                &event.context,
                event.priority,
            )?,
        };

        info!("Captured new {}: {}", event.event_type, entry.id);
        Ok(Some(entry))
    }
}

/// Classify tool into an area
fn classify_tool_area(tool_name: &str) -> String {
    match tool_name {
        "read_file" | "write_file" | "append_file" | "delete_file"
        | "list_directory" | "create_directory" | "file_info"
        | "search_files" | "find_files" | "glob" => "filesystem".to_string(),

        "execute_command" => "shell_execution".to_string(),

        "fetch_url" => "web_access".to_string(),

        "search_content" => "code_search".to_string(),

        "orchestrate_task" | "spawn_agents" => "orchestration".to_string(),

        "create_skill" | "use_skill" | "list_skills" => "skills".to_string(),

        "edit_source" | "view_source" | "rebuild_self"
        | "self_diagnose" | "self_repair" => "self_modification".to_string(),

        _ => "general".to_string(),
    }
}

/// Truncate a string to max length with ellipsis
fn truncate(s: &str, max_len: usize) -> String {
    crate::truncate_safe(s, max_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_tool_area() {
        assert_eq!(classify_tool_area("read_file"), "filesystem");
        assert_eq!(classify_tool_area("execute_command"), "shell_execution");
        assert_eq!(classify_tool_area("fetch_url"), "web_access");
        assert_eq!(classify_tool_area("unknown_tool"), "general");
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world foo bar", 10), "hello w...");
    }
}
