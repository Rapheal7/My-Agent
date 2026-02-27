//! Bootstrap Context Files - persistent evolving context loaded at session start
//!
//! Manages SOUL.md, MEMORY.md, TOOLS.md, AGENTS.md, and LEARNINGS.md
//! under ~/.local/share/my-agent/ to give the agent persistent context.

use anyhow::{Context, Result};
use std::path::PathBuf;
use tracing::{info, debug};

/// Bootstrap context file names
const BOOTSTRAP_FILES: &[(&str, &str)] = &[
    ("SOUL.md", "Personality, behavioral rules, and preferences"),
    ("MEMORY.md", "Persistent facts, user preferences, learned knowledge"),
    ("TOOLS.md", "Tool documentation, known issues, usage tips"),
    ("AGENTS.md", "Agent configurations, orchestration strategies"),
    ("LEARNINGS.md", "Promoted learnings from the self-improvement system"),
];

/// Manages bootstrap context files
pub struct BootstrapContext {
    base_dir: PathBuf,
}

impl BootstrapContext {
    /// Create a new bootstrap context at the default data directory
    pub fn new() -> Result<Self> {
        let base_dir = crate::config::data_dir()?;
        std::fs::create_dir_all(&base_dir)
            .context("Failed to create data directory")?;
        Ok(Self { base_dir })
    }

    /// Create with custom directory
    pub fn with_dir(base_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&base_dir)
            .context("Failed to create bootstrap directory")?;
        Ok(Self { base_dir })
    }

    /// Load all bootstrap files and concatenate into a context block
    pub fn load_all(&self) -> String {
        let mut context = String::with_capacity(8192);
        context.push_str("# Bootstrap Context\n\n");
        context.push_str("_Persistent knowledge loaded from previous sessions._\n\n");

        for (filename, description) in BOOTSTRAP_FILES {
            let path = self.base_dir.join(filename);
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        context.push_str(&format!("## {} ({})\n\n", filename, description));
                        context.push_str(trimmed);
                        context.push_str("\n\n");
                    }
                }
            }
        }

        // Load daily logs for temporal context
        if let Ok(log_mgr) = crate::memory::daily_log::DailyLogManager::new() {
            let recent = log_mgr.load_recent_context();
            if !recent.trim().is_empty() {
                context.push_str("## Recent Activity (Daily Logs)\n\n");
                context.push_str(recent.trim());
                context.push_str("\n\n");
            }
        }

        if context.len() < 80 {
            // No meaningful content loaded
            return String::new();
        }

        context
    }

    /// Load a specific bootstrap file
    pub fn load_file(&self, filename: &str) -> Result<String> {
        let path = self.base_dir.join(filename);
        if path.exists() {
            Ok(std::fs::read_to_string(&path)?)
        } else {
            Ok(String::new())
        }
    }

    /// Update a section within a bootstrap file
    pub fn update_section(&self, filename: &str, section: &str, content: &str) -> Result<()> {
        let path = self.base_dir.join(filename);
        let existing = if path.exists() {
            std::fs::read_to_string(&path)?
        } else {
            String::new()
        };

        let section_header = format!("### {}", section);
        let section_end = "\n### ";

        let new_content = if let Some(start) = existing.find(&section_header) {
            // Replace existing section
            let after_header = start + section_header.len();
            let end = existing[after_header..]
                .find(section_end)
                .map(|e| after_header + e)
                .unwrap_or(existing.len());

            format!(
                "{}{}\n\n{}\n\n{}",
                &existing[..start],
                section_header,
                content,
                &existing[end..]
            )
        } else {
            // Append new section
            format!("{}\n{}\n\n{}\n", existing, section_header, content)
        };

        std::fs::write(&path, new_content.trim_end())?;
        info!("Updated section '{}' in {}", section, filename);
        Ok(())
    }

    /// Get the full path to a bootstrap file
    pub fn file_path(&self, filename: &str) -> PathBuf {
        self.base_dir.join(filename)
    }

    /// Append content to a bootstrap file
    pub fn append_to_file(&self, filename: &str, content: &str) -> Result<()> {
        let path = self.base_dir.join(filename);
        let existing = if path.exists() {
            std::fs::read_to_string(&path)?
        } else {
            self.default_header(filename)
        };

        let new_content = format!("{}\n\n{}\n", existing.trim_end(), content);
        std::fs::write(&path, new_content)?;
        debug!("Appended to {}", filename);
        Ok(())
    }

    /// Remove a specific entry from a bootstrap file (by ID marker)
    pub fn remove_entry(&self, filename: &str, entry_id: &str) -> Result<bool> {
        let path = self.base_dir.join(filename);
        if !path.exists() {
            return Ok(false);
        }

        let content = std::fs::read_to_string(&path)?;
        let marker = format!("<!-- {} -->", entry_id);

        if !content.contains(&marker) {
            return Ok(false);
        }

        // Remove the block between markers
        let start_marker = format!("<!-- {} -->\n", entry_id);
        let end_marker = format!("<!-- /{} -->", entry_id);

        let new_content = if let (Some(start), Some(end)) = (content.find(&start_marker), content.find(&end_marker)) {
            let end_pos = end + end_marker.len();
            let end_pos = content[end_pos..].find('\n').map(|p| end_pos + p + 1).unwrap_or(end_pos);
            format!("{}{}", &content[..start], &content[end_pos..])
        } else {
            // Simple removal of the marker line and content until next marker or section
            content.replace(&marker, "")
        };

        std::fs::write(&path, new_content.trim())?;
        info!("Removed entry {} from {}", entry_id, filename);
        Ok(true)
    }

    /// Seed default bootstrap files from existing system_prompts.rs and personality.rs
    pub fn seed_defaults(&self) -> Result<()> {
        for (filename, _description) in BOOTSTRAP_FILES {
            let path = self.base_dir.join(filename);
            if !path.exists() || std::fs::read_to_string(&path)?.trim().is_empty() {
                let content = self.default_content(filename);
                std::fs::write(&path, content)?;
                info!("Seeded default {}", filename);
            }
        }
        Ok(())
    }

    /// Get default header for a file
    fn default_header(&self, filename: &str) -> String {
        match filename {
            "SOUL.md" => "# Soul\n\nPersonality, behavioral rules, and preferences.\n".to_string(),
            "MEMORY.md" => "# Memory\n\nPersistent facts and learned knowledge.\n".to_string(),
            "TOOLS.md" => "# Tools\n\nTool documentation, known issues, and usage tips.\n".to_string(),
            "AGENTS.md" => "# Agents\n\nAgent configurations and orchestration strategies.\n".to_string(),
            "LEARNINGS.md" => "# Learnings\n\nPromoted learnings from self-improvement.\n".to_string(),
            _ => format!("# {}\n", filename),
        }
    }

    /// Get default content for a bootstrap file
    fn default_content(&self, filename: &str) -> String {
        match filename {
            "SOUL.md" => {
                let personality = crate::soul::Personality::load().unwrap_or_default();
                format!(
                    "# Soul\n\n\
                     ## Identity\n\n\
                     Name: {}\n\
                     Traits: {}\n\n\
                     ## Behavioral Rules\n\n\
                     - Be helpful and precise\n\
                     - Confirm before destructive operations\n\
                     - Learn from corrections\n\
                     - Adapt to user preferences\n",
                    personality.name,
                    personality.traits.join(", ")
                )
            }
            "MEMORY.md" => {
                "# Memory\n\n\
                 ## User Preferences\n\n\
                 _No preferences recorded yet._\n\n\
                 ## Known Facts\n\n\
                 _No facts recorded yet._\n"
                    .to_string()
            }
            "TOOLS.md" => {
                "# Tools\n\n\
                 ## Tool Tips\n\n\
                 - read_file: Supports ~ for home directory expansion\n\
                 - execute_command: Requires user approval\n\
                 - write_file: Creates parent directories automatically\n\n\
                 ## Known Issues\n\n\
                 _No known issues._\n"
                    .to_string()
            }
            "AGENTS.md" => {
                "# Agents\n\n\
                 ## Available Roles\n\n\
                 - **code**: Code generation and modification\n\
                 - **research**: Information gathering and analysis\n\
                 - **reasoning**: Complex reasoning and planning\n\
                 - **utility**: File operations and general tasks\n\n\
                 ## Orchestration Tips\n\n\
                 - Use code agent for implementation tasks\n\
                 - Use research agent for documentation lookup\n\
                 - Spawn multiple agents for parallel subtasks\n"
                    .to_string()
            }
            "LEARNINGS.md" => {
                "# Learnings\n\n\
                 _Promoted learnings will appear here as the agent improves._\n"
                    .to_string()
            }
            _ => self.default_header(filename),
        }
    }

    /// Get the base directory
    pub fn base_dir(&self) -> &PathBuf {
        &self.base_dir
    }

    /// Check if bootstrap files exist
    pub fn is_initialized(&self) -> bool {
        BOOTSTRAP_FILES.iter().all(|(f, _)| self.base_dir.join(f).exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bootstrap_files_list() {
        assert_eq!(BOOTSTRAP_FILES.len(), 5);
        assert_eq!(BOOTSTRAP_FILES[0].0, "SOUL.md");
    }
}
