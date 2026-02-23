//! Self-Improving Learning System
//!
//! Captures learnings, errors, and feature requests from interactions,
//! stores them in structured flat files, and promotes validated patterns
//! to permanent bootstrap context.

pub mod store;
pub mod detector;
pub mod bootstrap;
pub mod promotion;

pub use store::{LearningStore, LearningEntry, EntryType, Priority, EntryStatus};
pub use detector::{LearningDetector, DetectedEvent};
pub use bootstrap::BootstrapContext;
pub use promotion::PromotionEngine;
