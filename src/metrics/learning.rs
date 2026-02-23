//! Learning and feedback loop
//!
//! Enables the agent to learn from execution outcomes and improve over time

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, debug};

use super::execution::{MetricsStore, ToolExecutionRecord};
use super::analysis::{ImprovementSuggestion, ImprovementCategory};

/// A learned lesson
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    /// Unique ID
    pub id: String,
    /// When learned
    pub learned_at: DateTime<Utc>,
    /// What was learned
    pub insight: String,
    /// Context where it applies
    pub context: String,
    /// How many times this lesson has been applied
    pub applications: u64,
    /// Confidence level (0.0-1.0)
    pub confidence: f64,
    /// Source of the lesson
    pub source: LessonSource,
    /// Related tools
    pub related_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LessonSource {
    /// Learned from repeated failures
    FailurePattern,
    /// Learned from successful strategies
    SuccessPattern,
    /// Learned from user feedback
    UserFeedback,
    /// Learned from self-analysis
    SelfAnalysis,
    /// Learned from external source
    External,
}

/// Outcome of a learning process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningOutcome {
    /// Whether learning was successful
    pub success: bool,
    /// Lessons extracted
    pub lessons: Vec<Lesson>,
    /// Suggested actions
    pub actions: Vec<String>,
    /// Confidence in the learning
    pub confidence: f64,
}

/// Feedback loop for continuous learning
pub struct FeedbackLoop {
    /// Metrics store
    metrics_store: MetricsStore,
    /// Learned lessons
    lessons: Arc<RwLock<Vec<Lesson>>>,
    /// Maximum lessons to keep
    max_lessons: usize,
}

impl FeedbackLoop {
    pub fn new(metrics_store: MetricsStore) -> Self {
        Self {
            metrics_store,
            lessons: Arc::new(RwLock::new(Vec::new())),
            max_lessons: 100,
        }
    }

    /// Learn from recent executions
    pub async fn learn_from_recent(&self) -> Result<LearningOutcome> {
        let records = self.metrics_store.get_recent_records().await;
        let mut lessons = Vec::new();
        let mut actions = Vec::new();

        // Analyze failures
        let failure_lessons = self.analyze_failures(&records).await;
        lessons.extend(failure_lessons);

        // Analyze successes
        let success_lessons = self.analyze_successes(&records).await;
        lessons.extend(success_lessons);

        // Generate actions based on lessons
        for lesson in &lessons {
            actions.push(format!("Consider: {} (applies to: {:?})", lesson.insight, lesson.related_tools));
        }

        let confidence = if lessons.is_empty() { 0.0 } else { 0.7 };

        Ok(LearningOutcome {
            success: !lessons.is_empty(),
            lessons,
            actions,
            confidence,
        })
    }

    async fn analyze_failures(&self, records: &[ToolExecutionRecord]) -> Vec<Lesson> {
        let mut lessons = Vec::new();

        // Group failures by tool
        let mut failures_by_tool: HashMap<String, Vec<&ToolExecutionRecord>> = HashMap::new();
        for record in records {
            if !record.success {
                failures_by_tool.entry(record.tool_name.clone())
                    .or_default()
                    .push(record);
            }
        }

        // Analyze each tool's failures
        for (tool, failures) in failures_by_tool {
            if failures.len() >= 3 {
                // Pattern: repeated failures
                let common_errors: Vec<_> = failures.iter()
                    .filter_map(|r| r.error.as_ref())
                    .take(3)
                    .collect();

                if !common_errors.is_empty() {
                    let lesson = Lesson {
                        id: uuid::Uuid::new_v4().to_string(),
                        learned_at: Utc::now(),
                        insight: format!(
                            "Tool '{}' has recurring issues. Common errors: {:?}",
                            tool,
                            common_errors.iter().map(|e| e.chars().take(50).collect::<String>()).collect::<Vec<_>>()
                        ),
                        context: format!("When using {} tool", tool),
                        applications: 0,
                        confidence: failures.len() as f64 / 10.0,
                        source: LessonSource::FailurePattern,
                        related_tools: vec![tool],
                    };
                    lessons.push(lesson);
                }
            }
        }

        lessons
    }

    async fn analyze_successes(&self, records: &[ToolExecutionRecord]) -> Vec<Lesson> {
        let mut lessons = Vec::new();

        // Find tools with consistent success
        let mut successes_by_tool: HashMap<String, Vec<&ToolExecutionRecord>> = HashMap::new();
        for record in records {
            if record.success {
                successes_by_tool.entry(record.tool_name.clone())
                    .or_default()
                    .push(record);
            }
        }

        for (tool, successes) in successes_by_tool {
            if successes.len() >= 5 {
                // High reliability tool
                let avg_duration: f64 = successes.iter()
                    .map(|r| r.duration_ms as f64)
                    .sum::<f64>() / successes.len() as f64;

                let lesson = Lesson {
                    id: uuid::Uuid::new_v4().to_string(),
                    learned_at: Utc::now(),
                    insight: format!(
                        "Tool '{}' is highly reliable ({} successful uses, avg {:.0}ms). Consider as primary choice for its category.",
                        tool, successes.len(), avg_duration
                    ),
                    context: format!("Tasks requiring {} capabilities", tool),
                    applications: 0,
                    confidence: successes.len() as f64 / 20.0,
                    source: LessonSource::SuccessPattern,
                    related_tools: vec![tool],
                };
                lessons.push(lesson);
            }
        }

        lessons
    }

    /// Learn from a specific execution
    pub async fn learn_from_execution(&self, record: &ToolExecutionRecord) -> Result<()> {
        let mut lessons = self.lessons.write().await;

        if !record.success {
            // Check if we've seen this error before
            if let Some(ref error) = record.error {
                let error_key = error.chars().take(50).collect::<String>();

                // Look for similar past lessons
                let similar = lessons.iter_mut()
                    .filter(|l| l.context.contains(&record.tool_name))
                    .find(|l| l.insight.contains(&error_key));

                if let Some(lesson) = similar {
                    lesson.applications += 1;
                    lesson.confidence = (lesson.confidence + 0.1).min(1.0);
                } else {
                    // Create new lesson
                    lessons.push(Lesson {
                        id: uuid::Uuid::new_v4().to_string(),
                        learned_at: Utc::now(),
                        insight: format!("Avoid {} in this context - error: {}", record.tool_name, error_key),
                        context: format!("When using {} tool", record.tool_name),
                        applications: 1,
                        confidence: 0.3,
                        source: LessonSource::FailurePattern,
                        related_tools: vec![record.tool_name.clone()],
                    });
                }
            }
        } else {
            // Success - update confidence in related lessons
            for lesson in lessons.iter_mut() {
                if lesson.related_tools.contains(&record.tool_name) && lesson.source == LessonSource::FailurePattern {
                    // This tool succeeded, so maybe our failure lesson wasn't universal
                    lesson.confidence *= 0.95;
                }
            }
        }

        // Trim if needed
        if lessons.len() > self.max_lessons {
            // Remove lowest confidence lessons
            lessons.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal));
            lessons.truncate(self.max_lessons);
        }

        Ok(())
    }

    /// Record user feedback
    pub async fn record_feedback(&self, tool_name: &str, rating: u8, comment: Option<&str>) -> Result<()> {
        let mut lessons = self.lessons.write().await;

        let insight = match rating {
            1 | 2 => format!("User dissatisfaction with {}: {}", tool_name, comment.unwrap_or("poor experience")),
            3 => format!("Neutral feedback on {}", tool_name),
            4 | 5 => format!("User satisfaction with {}: {}", tool_name, comment.unwrap_or("good experience")),
            _ => "Invalid rating".to_string(),
        };

        lessons.push(Lesson {
            id: uuid::Uuid::new_v4().to_string(),
            learned_at: Utc::now(),
            insight,
            context: format!("User feedback on {} tool", tool_name),
            applications: 1,
            confidence: 0.8, // User feedback is high confidence
            source: LessonSource::UserFeedback,
            related_tools: vec![tool_name.to_string()],
        });

        info!("Recorded user feedback for {}: {}/5", tool_name, rating);
        Ok(())
    }

    /// Get applicable lessons for a context
    pub async fn get_applicable_lessons(&self, context: &str) -> Vec<Lesson> {
        let lessons = self.lessons.read().await;

        lessons.iter()
            .filter(|l| {
                l.context.to_lowercase().contains(&context.to_lowercase()) ||
                l.related_tools.iter().any(|t| context.contains(t))
            })
            .filter(|l| l.confidence > 0.3) // Only confident lessons
            .cloned()
            .collect()
    }

    /// Get all lessons
    pub async fn get_all_lessons(&self) -> Vec<Lesson> {
        self.lessons.read().await.clone()
    }

    /// Convert lessons to improvement suggestions
    pub async fn lessons_to_suggestions(&self) -> Vec<ImprovementSuggestion> {
        let lessons = self.lessons.read().await;

        lessons.iter()
            .filter(|l| l.confidence > 0.5)
            .map(|l| ImprovementSuggestion {
                category: match l.source {
                    LessonSource::FailurePattern => ImprovementCategory::ErrorHandling,
                    LessonSource::SuccessPattern => ImprovementCategory::ToolUsage,
                    LessonSource::UserFeedback => ImprovementCategory::UserExperience,
                    LessonSource::SelfAnalysis => ImprovementCategory::SelfKnowledge,
                    LessonSource::External => ImprovementCategory::Reliability,
                },
                priority: (l.confidence * 5.0) as u8,
                title: format!("Lesson: {}", l.insight.chars().take(50).collect::<String>()),
                description: l.insight.clone(),
                action: format!("Apply insight in context: {}", l.context),
                expected_impact: format!("Confidence: {:.0}%", l.confidence * 100.0),
                related_to: l.related_tools.first().cloned(),
            })
            .collect()
    }

    /// Save lessons to memory
    pub async fn save(&self) -> Result<()> {
        let lessons = self.lessons.read().await;
        let json = serde_json::to_string_pretty(&*lessons)?;

        let path = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("my-agent")
            .join("lessons.json");

        tokio::fs::create_dir_all(path.parent().unwrap()).await?;
        tokio::fs::write(&path, json).await?;

        info!("Saved {} lessons to {:?}", lessons.len(), path);
        Ok(())
    }

    /// Load lessons from memory
    pub async fn load(&self) -> Result<()> {
        let path = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("my-agent")
            .join("lessons.json");

        if path.exists() {
            let json = tokio::fs::read_to_string(&path).await?;
            let loaded: Vec<Lesson> = serde_json::from_str(&json)?;
            let mut lessons = self.lessons.write().await;
            *lessons = loaded;

            info!("Loaded {} lessons from {:?}", lessons.len(), path);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_learn_from_failures() {
        let store = MetricsStore::new();
        let feedback = FeedbackLoop::new(store);

        // Record some failures
        for _ in 0..3 {
            let mut record = ToolExecutionRecord::new("test_tool");
            record.complete(false, Some("Test error".to_string()));
            feedback.learn_from_execution(&record).await.unwrap();
        }

        let lessons = feedback.get_all_lessons().await;
        assert!(!lessons.is_empty());
    }
}