//! Self-improvement metrics and analysis module
//!
//! This module provides the agent with capabilities to:
//! - Track execution metrics (success/failure rates, timing)
//! - Analyze patterns in its own behavior
//! - Learn from outcomes and improve over time
//! - Reflect on performance and suggest improvements

pub mod execution;
pub mod analysis;
pub mod learning;

pub use execution::{ExecutionMetrics, MetricsStore, ToolExecutionRecord};
pub use analysis::{SelfAnalyzer, PerformanceReport, ImprovementSuggestion};
pub use learning::{FeedbackLoop, Lesson, LearningOutcome};