//! Promotion Engine - promotes validated learnings to permanent bootstrap context
//!
//! Learnings that prove correct across multiple interactions get promoted
//! to the appropriate bootstrap file, making them part of permanent context.

use anyhow::Result;
use std::sync::Arc;
use tracing::{info, debug};

use super::store::{LearningStore, LearningEntry, EntryStatus, Priority};
use super::bootstrap::BootstrapContext;

/// Minimum occurrences for auto-promotion eligibility
const MIN_OCCURRENCES_FOR_PROMOTION: u32 = 3;

/// The promotion engine
pub struct PromotionEngine {
    store: Arc<LearningStore>,
    bootstrap: Arc<BootstrapContext>,
}

impl PromotionEngine {
    /// Create a new promotion engine
    pub fn new(store: Arc<LearningStore>, bootstrap: Arc<BootstrapContext>) -> Self {
        Self { store, bootstrap }
    }

    /// Find entries eligible for promotion
    pub fn check_promotable(&self) -> Result<Vec<LearningEntry>> {
        let validated = self.store.get_by_status(&EntryStatus::Validated)?;
        let promotable: Vec<LearningEntry> = validated
            .into_iter()
            .filter(|e| {
                e.occurrences >= MIN_OCCURRENCES_FOR_PROMOTION
                    && e.priority >= Priority::Medium
            })
            .collect();

        debug!("Found {} promotable entries", promotable.len());
        Ok(promotable)
    }

    /// Promote a specific entry to the appropriate bootstrap file
    pub fn promote(&self, entry_id: &str) -> Result<()> {
        let entry = self.store.get_by_id(entry_id)?
            .ok_or_else(|| anyhow::anyhow!("Entry not found: {}", entry_id))?;

        if entry.status == EntryStatus::Promoted {
            info!("Entry {} is already promoted", entry_id);
            return Ok(());
        }

        // Determine target file based on area
        let target_file = self.classify_target(&entry);

        // Format the entry for the bootstrap file
        let formatted = format!(
            "<!-- {} -->\n### {} ({})\n\n{}\n\n_Source: {} | Occurrences: {} | Area: {}_\n<!-- /{} -->",
            entry.id,
            entry.title,
            entry.priority,
            entry.description,
            entry.id,
            entry.occurrences,
            entry.area,
            entry.id
        );

        // Append to bootstrap file
        self.bootstrap.append_to_file(target_file, &formatted)?;

        // Update entry status
        self.store.promote(entry_id)?;

        info!(
            "Promoted {} to {} (occurrences: {}, priority: {})",
            entry_id, target_file, entry.occurrences, entry.priority
        );

        Ok(())
    }

    /// Demote an entry (remove from bootstrap, set status back)
    pub fn demote(&self, entry_id: &str) -> Result<()> {
        let entry = self.store.get_by_id(entry_id)?
            .ok_or_else(|| anyhow::anyhow!("Entry not found: {}", entry_id))?;

        if entry.status != EntryStatus::Promoted {
            info!("Entry {} is not promoted, nothing to demote", entry_id);
            return Ok(());
        }

        // Remove from all bootstrap files
        for file in &["SOUL.md", "MEMORY.md", "TOOLS.md", "AGENTS.md", "LEARNINGS.md"] {
            let _ = self.bootstrap.remove_entry(file, entry_id);
        }

        // Update status back to Validated
        self.store.update_status(entry_id, EntryStatus::Validated)?;

        info!("Demoted {} back to validated", entry_id);
        Ok(())
    }

    /// Run a full promotion cycle (check + promote all eligible)
    pub fn run_promotion_cycle(&self) -> Result<usize> {
        let promotable = self.check_promotable()?;
        let count = promotable.len();

        if count == 0 {
            debug!("No entries eligible for promotion");
            return Ok(0);
        }

        info!("Running promotion cycle: {} entries eligible", count);

        let mut promoted = 0;
        for entry in promotable {
            match self.promote(&entry.id) {
                Ok(()) => promoted += 1,
                Err(e) => {
                    tracing::warn!("Failed to promote {}: {}", entry.id, e);
                }
            }
        }

        info!("Promotion cycle complete: {} of {} entries promoted", promoted, count);
        Ok(promoted)
    }

    /// Classify which bootstrap file an entry should go to
    fn classify_target(&self, entry: &LearningEntry) -> &str {
        match entry.area.as_str() {
            // Tool-related learnings
            "filesystem" | "shell_execution" | "web_access"
            | "code_search" | "skills" | "tool_usage" => "TOOLS.md",

            // Behavioral/personality learnings
            "user_correction" | "communication" | "behavior"
            | "personality" | "style" => "SOUL.md",

            // Orchestration/agent learnings
            "orchestration" | "agent_routing" | "delegation"
            | "multi_agent" => "AGENTS.md",

            // Performance and general knowledge
            "performance" | "self_modification" => "TOOLS.md",

            // Missing capabilities -> feature requests -> learnings
            "missing_capability" => "LEARNINGS.md",

            // Default to MEMORY.md for general knowledge
            _ => "MEMORY.md",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_min_occurrences() {
        assert_eq!(MIN_OCCURRENCES_FOR_PROMOTION, 3);
    }
}
