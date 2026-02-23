//! Dynamic skill loading
//!
//! Loads skill definitions from disk and compiles them for execution.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use super::registry::{SkillMeta, Skill, SkillResult, SkillContext, SkillRegistry};
use super::generator::{GeneratedSkill, SkillGenerator};

/// Skill loader for loading skills from disk
pub struct SkillLoader {
    /// Skill registry to load into
    registry: SkillRegistry,
    /// Skill generator for compiling skills
    generator: SkillGenerator,
}

impl SkillLoader {
    /// Create a new skill loader
    pub fn new() -> Self {
        Self {
            registry: SkillRegistry::new(),
            generator: SkillGenerator::new(),
        }
    }

    /// Create loader with existing registry
    pub fn with_registry(registry: SkillRegistry) -> Self {
        Self {
            registry,
            generator: SkillGenerator::new(),
        }
    }

    /// Get the registry
    pub fn registry(&self) -> &SkillRegistry {
        &self.registry
    }

    /// Load all skills from the skills directory
    pub fn load_all(&self) -> Result<Vec<String>> {
        let skill_ids = self.registry.list_saved_skills()?;
        let mut loaded = Vec::new();

        for id in skill_ids {
            match self.load(&id) {
                Ok(_) => loaded.push(id),
                Err(e) => warn!("Failed to load skill '{}': {}", id, e),
            }
        }

        info!("Loaded {} skills", loaded.len());
        Ok(loaded)
    }

    /// Load a skill by ID
    pub fn load(&self, id: &str) -> Result<()> {
        let meta = self.registry.load_skill(id)?;
        let skill = self.compile_meta(&meta)?;
        self.registry.register(skill)?;
        info!("Loaded skill: {}", id);
        Ok(())
    }

    /// Load a skill from a file path
    pub fn load_from_file(&self, path: &Path) -> Result<()> {
        let content = std::fs::read_to_string(path)?;
        let generated: GeneratedSkill = serde_json::from_str(&content)?;

        let skill = self.generator.compile_skill(&generated)?;
        self.registry.register(skill)?;

        // Save to registry
        self.registry.save_skill(&generated.meta)?;

        info!("Loaded skill from file: {}", path.display());
        Ok(())
    }

    /// Compile skill metadata into an executable skill
    fn compile_meta(&self, meta: &SkillMeta) -> Result<Skill> {
        // Create a stub skill based on metadata
        // In a full implementation, this would load and compile actual skill code
        let meta_for_closure = meta.clone();
        let meta_for_call = meta.clone();

        let skill = Skill::new(meta_for_closure, move |params, ctx| {
            execute_skill_stub(&meta_for_call, params, ctx)
        });

        Ok(skill)
    }

    /// Install a skill from a URL or package reference
    pub async fn install(&self, source: &str) -> Result<String> {
        // Check if it's a URL
        if source.starts_with("http://") || source.starts_with("https://") {
            return self.install_from_url(source).await;
        }

        // Check if it's a local file
        let path = PathBuf::from(source);
        if path.exists() {
            self.load_from_file(&path)?;
            return Ok(path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| source.to_string()));
        }

        // Try to generate a skill from the description
        self.install_from_description(source).await
    }

    /// Install a skill from a URL
    async fn install_from_url(&self, url: &str) -> Result<String> {
        let client = reqwest::Client::new();
        let response = client.get(url).send().await?;

        if !response.status().is_success() {
            bail!("Failed to fetch skill from URL: {}", response.status());
        }

        let content = response.text().await?;
        let generated: GeneratedSkill = serde_json::from_str(&content)?;

        let id = generated.meta.id.clone();
        let skill = self.generator.compile_skill(&generated)?;

        self.registry.register(skill)?;
        self.registry.save_skill(&generated.meta)?;

        info!("Installed skill from URL: {}", id);
        Ok(id)
    }

    /// Generate and install a skill from a description
    async fn install_from_description(&self, description: &str) -> Result<String> {
        use super::generator::GenerationRequest;

        let request = GenerationRequest {
            description: description.to_string(),
            name: None,
            category: None,
            permissions: vec![],
            examples: vec![],
        };

        let generated = self.generator.generate(request).await?;
        let id = generated.meta.id.clone();

        let skill = self.generator.compile_skill(&generated)?;
        self.registry.register(skill)?;
        self.registry.save_skill(&generated.meta)?;

        info!("Installed generated skill: {}", id);
        Ok(id)
    }

    /// Remove a skill
    pub fn uninstall(&self, id: &str) -> Result<()> {
        self.registry.unregister(id)?;
        self.registry.delete_skill(id)?;
        info!("Uninstalled skill: {}", id);
        Ok(())
    }
}

impl Default for SkillLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// Execute a skill stub (placeholder implementation)
fn execute_skill_stub(
    meta: &SkillMeta,
    params: std::collections::HashMap<String, String>,
    _ctx: &SkillContext,
) -> Result<SkillResult> {
    // This is a placeholder - in a full implementation,
    // we would execute actual skill code

    let params_json = serde_json::to_string_pretty(&params)?;

    let output = format!(
        "Skill: {} (v{})\n{}\n\nParameters:\n{}",
        meta.name,
        meta.version,
        meta.description,
        params_json
    );

    Ok(SkillResult {
        success: true,
        output,
        error: None,
        duration_ms: 0,
    })
}

/// Skill definition file format
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillDefinition {
    /// Skill metadata
    pub meta: SkillMeta,
    /// Skill code (if embedded)
    pub code: Option<String>,
    /// URL to load code from
    pub code_url: Option<String>,
}

impl SkillDefinition {
    /// Load a skill definition from a file
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let def: SkillDefinition = serde_json::from_str(&content)?;
        Ok(def)
    }

    /// Save a skill definition to a file
    pub fn to_file(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loader_creation() {
        let loader = SkillLoader::new();
        assert!(loader.registry().list().is_empty());
    }

    #[test]
    fn test_skill_definition() {
        let meta = SkillMeta {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: "Test skill".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            category: super::super::registry::SkillCategory::Utility,
            permissions: vec![],
            parameters: vec![],
            builtin: false,
            tags: vec![],
        };

        let def = SkillDefinition {
            meta,
            code: Some("def execute(): pass".to_string()),
            code_url: None,
        };

        assert_eq!(def.meta.id, "test");
        assert!(def.code.is_some());
    }
}
