//! Self-analysis capabilities
//!
//! Analyzes execution data to identify patterns and suggest improvements

use anyhow::Result;
use chrono::Timelike;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, debug};

use super::execution::{ExecutionMetrics, MetricsStore, ToolMetrics};

/// A suggestion for improvement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImprovementSuggestion {
    /// Category of improvement
    pub category: ImprovementCategory,
    /// Priority level (1-5, 5 being highest)
    pub priority: u8,
    /// Title of the suggestion
    pub title: String,
    /// Detailed description
    pub description: String,
    /// Specific action to take
    pub action: String,
    /// Expected impact
    pub expected_impact: String,
    /// Related tool or area
    pub related_to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ImprovementCategory {
    /// Tool usage optimization
    ToolUsage,
    /// Error handling improvement
    ErrorHandling,
    /// Performance optimization
    Performance,
    /// Reliability improvement
    Reliability,
    /// User experience improvement
    UserExperience,
    /// Self-knowledge/learning
    SelfKnowledge,
}

/// Performance report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceReport {
    /// Overall health score (0-100)
    pub health_score: u8,
    /// Summary of performance
    pub summary: String,
    /// Top performing tools
    pub top_performers: Vec<String>,
    /// Tools needing attention
    pub attention_needed: Vec<(String, String)>,
    /// Improvement suggestions
    pub suggestions: Vec<ImprovementSuggestion>,
    /// Generated at
    pub generated_at: chrono::DateTime<chrono::Utc>,
}

/// Self-analyzer
pub struct SelfAnalyzer {
    /// Reference to metrics store
    metrics_store: MetricsStore,
}

impl SelfAnalyzer {
    pub fn new(metrics_store: MetricsStore) -> Self {
        Self { metrics_store }
    }

    /// Analyze current performance and generate report
    pub async fn analyze(&self) -> Result<PerformanceReport> {
        let metrics = self.metrics_store.get_metrics().await;

        // Calculate health score
        let health_score = self.calculate_health_score(&metrics);

        // Identify top performers
        let top_performers = self.identify_top_performers(&metrics);

        // Identify tools needing attention
        let attention_needed = self.identify_attention_needed(&metrics);

        // Generate suggestions
        let suggestions = self.generate_suggestions(&metrics);

        // Create summary
        let summary = self.create_summary(&metrics, health_score);

        Ok(PerformanceReport {
            health_score,
            summary,
            top_performers,
            attention_needed,
            suggestions,
            generated_at: chrono::Utc::now(),
        })
    }

    /// Calculate overall health score (0-100)
    fn calculate_health_score(&self, metrics: &ExecutionMetrics) -> u8 {
        if metrics.total_executions == 0 {
            return 100; // No data = perfect health (neutral state)
        }

        // Weight factors:
        // - Success rate: 50%
        // - Tool diversity: 20%
        // - Error recovery: 30%

        let success_score = metrics.overall_success_rate * 50.0;

        // Tool diversity: more tools used = better adaptability
        let diversity_score = (metrics.tools.len() as f64 / 10.0).min(1.0) * 20.0;

        // Error recovery: if errors exist but success rate is still good, that's recovery
        let total_errors: u64 = metrics.tools.values().map(|t| t.failed).sum();
        let recovery_score = if total_errors > 0 {
            (metrics.overall_success_rate * 30.0).min(30.0)
        } else {
            30.0 // No errors = full recovery score
        };

        (success_score + diversity_score + recovery_score).min(100.0) as u8
    }

    fn identify_top_performers(&self, metrics: &ExecutionMetrics) -> Vec<String> {
        let mut performers: Vec<_> = metrics.tools.iter()
            .filter(|(_, m)| m.total_executions >= 3 && m.success_rate() >= 0.9)
            .map(|(name, m)| (name.clone(), m.total_executions, m.success_rate()))
            .collect();

        performers.sort_by(|a, b| {
            b.2.partial_cmp(&a.2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.1.cmp(&a.1))
        });

        performers.iter().take(5).map(|(name, _, _)| name.clone()).collect()
    }

    fn identify_attention_needed(&self, metrics: &ExecutionMetrics) -> Vec<(String, String)> {
        let mut needs_attention = Vec::new();

        for (name, tool_metrics) in &metrics.tools {
            if tool_metrics.total_executions < 3 {
                continue; // Not enough data
            }

            // Low success rate
            if tool_metrics.success_rate() < 0.7 {
                needs_attention.push((
                    name.clone(),
                    format!("Low success rate: {:.1}%", tool_metrics.success_rate() * 100.0)
                ));
            }

            // Slow execution
            if tool_metrics.avg_duration_ms > 5000.0 {
                needs_attention.push((
                    name.clone(),
                    format!("Slow average execution: {:.0}ms", tool_metrics.avg_duration_ms)
                ));
            }

            // Frequent errors
            if tool_metrics.failed > tool_metrics.successful {
                needs_attention.push((
                    name.clone(),
                    format!("More failures than successes: {} failures", tool_metrics.failed)
                ));
            }
        }

        needs_attention
    }

    fn generate_suggestions(&self, metrics: &ExecutionMetrics) -> Vec<ImprovementSuggestion> {
        let mut suggestions = Vec::new();

        for (name, tool_metrics) in &metrics.tools {
            // Suggest improvements for low success rate tools
            if tool_metrics.success_rate() < 0.5 && tool_metrics.total_executions >= 3 {
                suggestions.push(ImprovementSuggestion {
                    category: ImprovementCategory::Reliability,
                    priority: 5,
                    title: format!("Investigate {} failures", name),
                    description: format!(
                        "Tool '{}' has {:.0}% success rate. Analyze failure patterns.",
                        name,
                        tool_metrics.success_rate() * 100.0
                    ),
                    action: format!("Review common errors and add error handling for: {:?}", tool_metrics.common_errors),
                    expected_impact: "Could improve success rate by 20-40%".to_string(),
                    related_to: Some(name.clone()),
                });
            }

            // Suggest optimization for slow tools
            if tool_metrics.avg_duration_ms > 3000.0 {
                suggestions.push(ImprovementSuggestion {
                    category: ImprovementCategory::Performance,
                    priority: 3,
                    title: format!("Optimize {} performance", name),
                    description: format!(
                        "Tool '{}' takes {:.0}ms on average. Consider optimization.",
                        name,
                        tool_metrics.avg_duration_ms
                    ),
                    action: "Profile slow operations and add caching where possible".to_string(),
                    expected_impact: "Could reduce execution time by 30-50%".to_string(),
                    related_to: Some(name.clone()),
                });
            }
        }

        // Suggest learning from underused tools
        let underused: Vec<_> = metrics.tools.iter()
            .filter(|(_, m)| m.total_executions < 5)
            .map(|(n, _)| n.as_str())
            .collect();

        if !underused.is_empty() && metrics.total_executions > 20 {
            suggestions.push(ImprovementSuggestion {
                category: ImprovementCategory::SelfKnowledge,
                priority: 2,
                title: "Explore underutilized capabilities".to_string(),
                description: format!("Several tools are rarely used: {:?}", underused),
                action: "Consider if these tools could solve recurring problems more efficiently".to_string(),
                expected_impact: "Better tool utilization and efficiency".to_string(),
                related_to: None,
            });
        }

        // Sort by priority
        suggestions.sort_by(|a, b| b.priority.cmp(&a.priority));
        suggestions
    }

    fn create_summary(&self, metrics: &ExecutionMetrics, health_score: u8) -> String {
        let total_tools = metrics.tools.len();
        let most_used = metrics.tools.iter()
            .max_by_key(|(_, m)| m.total_executions)
            .map(|(n, _)| n.clone())
            .unwrap_or_else(|| "none".to_string());

        format!(
            "Health Score: {}/100 | {} executions across {} tools | Most used: {}",
            health_score, metrics.total_executions, total_tools, most_used
        )
    }

    /// Analyze patterns in execution history
    pub async fn analyze_patterns(&self) -> Result<Vec<ExecutionPattern>> {
        let records = self.metrics_store.get_recent_records().await;
        let mut patterns = Vec::new();

        // Pattern 1: Tool sequences
        let sequences = self.find_tool_sequences(&records);
        if !sequences.is_empty() {
            patterns.push(ExecutionPattern {
                pattern_type: PatternType::ToolSequence,
                description: "Common tool sequences identified".to_string(),
                occurrences: sequences.len() as u64,
                details: serde_json::to_string(&sequences).unwrap_or_default(),
            });
        }

        // Pattern 2: Time-of-day patterns
        let time_patterns = self.analyze_time_patterns(&records);
        if time_patterns.is_some() {
            patterns.push(ExecutionPattern {
                pattern_type: PatternType::TimeBased,
                description: "Execution patterns by time of day".to_string(),
                occurrences: records.len() as u64,
                details: serde_json::to_string(&time_patterns).unwrap_or_default(),
            });
        }

        // Pattern 3: Error patterns
        let error_patterns = self.analyze_error_patterns(&records);
        if !error_patterns.is_empty() {
            patterns.push(ExecutionPattern {
                pattern_type: PatternType::ErrorCluster,
                description: format!("{} error clusters identified", error_patterns.len()),
                occurrences: error_patterns.len() as u64,
                details: serde_json::to_string(&error_patterns).unwrap_or_default(),
            });
        }

        Ok(patterns)
    }

    fn find_tool_sequences(&self, records: &[super::execution::ToolExecutionRecord]) -> Vec<Vec<String>> {
        // Find common 3-tool sequences
        let mut sequences: HashMap<Vec<String>, u64> = HashMap::new();

        for window in records.windows(3) {
            let seq: Vec<String> = window.iter().map(|r| r.tool_name.clone()).collect();
            *sequences.entry(seq).or_default() += 1;
        }

        sequences.into_iter()
            .filter(|(_, count)| *count >= 2)
            .map(|(seq, _)| seq)
            .collect()
    }

    fn analyze_time_patterns(&self, records: &[super::execution::ToolExecutionRecord]) -> Option<HashMap<u8, u64>> {
        if records.is_empty() {
            return None;
        }

        let mut hour_counts: HashMap<u8, u64> = HashMap::new();
        for record in records {
            let hour = record.started_at.hour() as u8;
            *hour_counts.entry(hour).or_default() += 1;
        }

        Some(hour_counts)
    }

    fn analyze_error_patterns(&self, records: &[super::execution::ToolExecutionRecord]) -> Vec<ErrorPattern> {
        let mut error_patterns: HashMap<String, Vec<String>> = HashMap::new();

        for record in records {
            if !record.success {
                if let Some(ref error) = record.error {
                    let error_key = error.chars().take(50).collect::<String>();
                    error_patterns.entry(error_key).or_default().push(record.tool_name.clone());
                }
            }
        }

        error_patterns.into_iter()
            .filter(|(_, tools)| tools.len() >= 2)
            .map(|(error, tools)| ErrorPattern {
                error_prefix: error,
                affected_tools: tools,
            })
            .collect()
    }
}

/// A detected execution pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPattern {
    pub pattern_type: PatternType,
    pub description: String,
    pub occurrences: u64,
    pub details: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PatternType {
    ToolSequence,
    TimeBased,
    ErrorCluster,
}

/// An error pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPattern {
    pub error_prefix: String,
    pub affected_tools: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_analyze_empty_metrics() {
        let store = MetricsStore::new();
        let analyzer = SelfAnalyzer::new(store);

        let report = analyzer.analyze().await.unwrap();
        assert_eq!(report.health_score, 100); // No data = perfect health
    }
}