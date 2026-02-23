//! Execution metrics tracking
//!
//! Records and analyzes tool execution data for self-improvement

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, debug};

/// A single tool execution record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionRecord {
    /// Unique ID for this execution
    pub id: String,
    /// Tool name that was executed
    pub tool_name: String,
    /// When execution started
    pub started_at: DateTime<Utc>,
    /// When execution ended
    pub ended_at: Option<DateTime<Utc>>,
    /// Whether execution succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Token usage if LLM was involved
    pub tokens_used: Option<u64>,
    /// Context hash for grouping similar executions
    pub context_hash: Option<String>,
    /// User feedback rating (1-5, if provided)
    pub user_rating: Option<u8>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

impl ToolExecutionRecord {
    pub fn new(tool_name: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            tool_name: tool_name.to_string(),
            started_at: Utc::now(),
            ended_at: None,
            success: false,
            error: None,
            duration_ms: 0,
            tokens_used: None,
            context_hash: None,
            user_rating: None,
            metadata: HashMap::new(),
        }
    }

    pub fn complete(&mut self, success: bool, error: Option<String>) {
        self.ended_at = Some(Utc::now());
        self.success = success;
        self.error = error;
        self.duration_ms = (self.ended_at.unwrap() - self.started_at)
            .num_milliseconds()
            .max(0) as u64;
    }
}

/// Aggregated metrics for a tool
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolMetrics {
    /// Total number of executions
    pub total_executions: u64,
    /// Number of successful executions
    pub successful: u64,
    /// Number of failed executions
    pub failed: u64,
    /// Average execution time in ms
    pub avg_duration_ms: f64,
    /// Min execution time
    pub min_duration_ms: u64,
    /// Max execution time
    pub max_duration_ms: u64,
    /// Total tokens used
    pub total_tokens: u64,
    /// Average user rating
    pub avg_rating: Option<f64>,
    /// Most common errors
    pub common_errors: Vec<(String, u64)>,
}

impl ToolMetrics {
    pub fn success_rate(&self) -> f64 {
        if self.total_executions == 0 {
            0.0
        } else {
            self.successful as f64 / self.total_executions as f64
        }
    }
}

/// Overall execution metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionMetrics {
    /// Per-tool metrics
    pub tools: HashMap<String, ToolMetrics>,
    /// Total executions across all tools
    pub total_executions: u64,
    /// Overall success rate
    pub overall_success_rate: f64,
    /// Session start time
    pub session_start: DateTime<Utc>,
    /// Last updated
    pub last_updated: DateTime<Utc>,
}

/// Persistent metrics store
pub struct MetricsStore {
    /// In-memory metrics
    metrics: Arc<RwLock<ExecutionMetrics>>,
    /// Database path for persistence
    db_path: PathBuf,
    /// Recent records for analysis
    recent_records: Arc<RwLock<Vec<ToolExecutionRecord>>>,
    /// Maximum records to keep in memory
    max_recent: usize,
}

impl MetricsStore {
    pub fn new() -> Self {
        let db_path = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("my-agent")
            .join("metrics.db");

        Self {
            metrics: Arc::new(RwLock::new(ExecutionMetrics {
                session_start: Utc::now(),
                last_updated: Utc::now(),
                ..Default::default()
            })),
            db_path,
            recent_records: Arc::new(RwLock::new(Vec::new())),
            max_recent: 1000,
        }
    }

    /// Record a tool execution
    pub async fn record(&self, record: ToolExecutionRecord) {
        let mut metrics = self.metrics.write().await;
        let mut recent = self.recent_records.write().await;

        // Update tool metrics
        let tool_metrics = metrics.tools.entry(record.tool_name.clone()).or_default();
        tool_metrics.total_executions += 1;

        if record.success {
            tool_metrics.successful += 1;
        } else {
            tool_metrics.failed += 1;
            if let Some(ref error) = record.error {
                // Track common errors (simplified - just keep top 5)
                let error_key = error.chars().take(100).collect::<String>();
                if let Some(pos) = tool_metrics.common_errors.iter().position(|(e, _)| e == &error_key) {
                    tool_metrics.common_errors[pos].1 += 1;
                } else if tool_metrics.common_errors.len() < 5 {
                    tool_metrics.common_errors.push((error_key, 1));
                }
            }
        }

        // Update duration stats
        if tool_metrics.total_executions == 1 {
            tool_metrics.avg_duration_ms = record.duration_ms as f64;
            tool_metrics.min_duration_ms = record.duration_ms;
            tool_metrics.max_duration_ms = record.duration_ms;
        } else {
            let n = tool_metrics.total_executions as f64;
            tool_metrics.avg_duration_ms =
                tool_metrics.avg_duration_ms * (n - 1.0) / n + record.duration_ms as f64 / n;
            tool_metrics.min_duration_ms = tool_metrics.min_duration_ms.min(record.duration_ms);
            tool_metrics.max_duration_ms = tool_metrics.max_duration_ms.max(record.duration_ms);
        }

        if let Some(tokens) = record.tokens_used {
            tool_metrics.total_tokens += tokens;
        }

        // Update overall metrics
        metrics.total_executions += 1;
        let total_successful: u64 = metrics.tools.values().map(|t| t.successful).sum();
        metrics.overall_success_rate = if metrics.total_executions > 0 {
            total_successful as f64 / metrics.total_executions as f64
        } else {
            0.0
        };
        metrics.last_updated = Utc::now();

        // Add to recent records
        recent.push(record);
        if recent.len() > self.max_recent {
            recent.remove(0);
        }

        debug!("Recorded execution for tool, total: {}", metrics.total_executions);
    }

    /// Get current metrics
    pub async fn get_metrics(&self) -> ExecutionMetrics {
        self.metrics.read().await.clone()
    }

    /// Get recent records
    pub async fn get_recent_records(&self) -> Vec<ToolExecutionRecord> {
        self.recent_records.read().await.clone()
    }

    /// Get metrics for a specific tool
    pub async fn get_tool_metrics(&self, tool_name: &str) -> Option<ToolMetrics> {
        let metrics = self.metrics.read().await;
        metrics.tools.get(tool_name).cloned()
    }

    /// Get most used tools
    pub async fn get_most_used_tools(&self, limit: usize) -> Vec<(String, ToolMetrics)> {
        let metrics = self.metrics.read().await;
        let mut tools: Vec<_> = metrics.tools.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        tools.sort_by(|a, b| b.1.total_executions.cmp(&a.1.total_executions));
        tools.into_iter().take(limit).collect()
    }

    /// Get tools with lowest success rates
    pub async fn get_problematic_tools(&self) -> Vec<(String, ToolMetrics)> {
        let metrics = self.metrics.read().await;
        let mut tools: Vec<_> = metrics.tools.iter()
            .filter(|(_, v)| v.total_executions >= 3) // Need enough data
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        tools.sort_by(|a, b| a.1.success_rate().partial_cmp(&b.1.success_rate()).unwrap_or(std::cmp::Ordering::Equal));
        tools
    }

    /// Save metrics to disk
    pub async fn save(&self) -> Result<()> {
        let metrics = self.metrics.read().await;
        let json = serde_json::to_string_pretty(&*metrics)?;
        tokio::fs::create_dir_all(self.db_path.parent().unwrap()).await?;
        tokio::fs::write(&self.db_path, json).await?;
        info!("Saved metrics to {:?}", self.db_path);
        Ok(())
    }

    /// Load metrics from disk
    pub async fn load(&self) -> Result<()> {
        if self.db_path.exists() {
            let json = tokio::fs::read_to_string(&self.db_path).await?;
            let loaded: ExecutionMetrics = serde_json::from_str(&json)?;
            let mut metrics = self.metrics.write().await;
            *metrics = loaded;
            info!("Loaded metrics from {:?}", self.db_path);
        }
        Ok(())
    }

    /// Clear all metrics
    pub async fn clear(&self) {
        let mut metrics = self.metrics.write().await;
        let mut recent = self.recent_records.write().await;
        *metrics = ExecutionMetrics {
            session_start: Utc::now(),
            last_updated: Utc::now(),
            ..Default::default()
        };
        recent.clear();
    }
}

impl Default for MetricsStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_record_execution() {
        let store = MetricsStore::new();

        let mut record = ToolExecutionRecord::new("test_tool");
        record.complete(true, None);

        store.record(record).await;

        let metrics = store.get_metrics().await;
        assert_eq!(metrics.total_executions, 1);
        assert!(metrics.tools.contains_key("test_tool"));
    }

    #[tokio::test]
    async fn test_success_rate() {
        let store = MetricsStore::new();

        // Record 3 successes and 1 failure
        for i in 0..4 {
            let mut record = ToolExecutionRecord::new("test_tool");
            record.complete(i < 3, if i >= 3 { Some("Error".to_string()) } else { None });
            store.record(record).await;
        }

        let tool_metrics = store.get_tool_metrics("test_tool").await.unwrap();
        assert_eq!(tool_metrics.success_rate(), 0.75);
    }
}