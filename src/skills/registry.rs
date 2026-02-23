//! Skill registry for managing installed skills
//!
//! Provides registration, discovery, and execution of skills.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::info;

/// Skill metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMeta {
    /// Unique skill identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Skill version
    pub version: String,
    /// Author information
    pub author: Option<String>,
    /// Skill category
    pub category: SkillCategory,
    /// Required permissions
    pub permissions: Vec<Permission>,
    /// Parameter schema (JSON Schema style)
    pub parameters: Vec<SkillParameter>,
    /// Whether this is a built-in skill
    pub builtin: bool,
    /// Tags for search/discovery
    pub tags: Vec<String>,
}

/// Skill category
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SkillCategory {
    /// File system operations
    Filesystem,
    /// Shell/command execution
    Shell,
    /// Web/API operations
    Web,
    /// Data processing
    Data,
    /// System operations
    System,
    /// Utility functions
    Utility,
    /// Custom/other
    Custom,
}

impl Default for SkillCategory {
    fn default() -> Self {
        Self::Utility
    }
}

/// Permission level required by a skill
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Permission {
    /// Read file system
    ReadFiles,
    /// Write file system
    WriteFiles,
    /// Execute shell commands
    ExecuteCommands,
    /// Network access
    NetworkAccess,
    /// Access environment variables
    ReadEnvironment,
    /// Modify system settings
    SystemModify,
}

/// Skill parameter definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillParameter {
    /// Parameter name
    pub name: String,
    /// Parameter type
    pub param_type: ParameterType,
    /// Whether this parameter is required
    pub required: bool,
    /// Default value (as string)
    pub default: Option<String>,
    /// Description
    pub description: String,
    /// Allowed values (for enum types)
    pub allowed_values: Option<Vec<String>>,
}

/// Parameter types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParameterType {
    String,
    Integer,
    Float,
    Boolean,
    Path,
    Url,
    Enum,
    Array,
    Object,
}

/// A skill execution context
#[derive(Debug, Clone)]
pub struct SkillContext {
    /// Working directory
    pub working_dir: PathBuf,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// Timeout in seconds
    pub timeout_secs: u64,
    /// Whether to require approval for risky operations
    pub require_approval: bool,
}

impl Default for SkillContext {
    fn default() -> Self {
        Self {
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            env: std::env::vars().collect(),
            timeout_secs: 60,
            require_approval: true,
        }
    }
}

/// Skill execution result
#[derive(Debug, Clone, Serialize)]
pub struct SkillResult {
    /// Whether execution succeeded
    pub success: bool,
    /// Output/result data
    pub output: String,
    /// Error message if failed
    pub error: Option<String>,
    /// Execution duration in milliseconds
    pub duration_ms: u64,
}

/// Type alias for skill executor function
type SkillExecutor = Arc<dyn Fn(HashMap<String, String>, &SkillContext) -> Result<SkillResult> + Send + Sync>;

/// A registered skill
pub struct Skill {
    /// Skill metadata
    pub meta: SkillMeta,
    /// Executor function
    executor: SkillExecutor,
}

impl Skill {
    /// Create a new skill
    pub fn new<F>(meta: SkillMeta, executor: F) -> Self
    where
        F: Fn(HashMap<String, String>, &SkillContext) -> Result<SkillResult> + Send + Sync + 'static,
    {
        Self {
            meta,
            executor: Arc::new(executor),
        }
    }

    /// Execute the skill
    pub fn execute(&self, params: HashMap<String, String>, ctx: &SkillContext) -> Result<SkillResult> {
        (self.executor)(params, ctx)
    }
}

/// Global skill registry
pub struct SkillRegistry {
    /// Registered skills
    skills: Arc<Mutex<HashMap<String, Skill>>>,
    /// Skills directory for persistent storage
    skills_dir: PathBuf,
}

impl SkillRegistry {
    /// Create a new skill registry
    pub fn new() -> Self {
        let skills_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("my-agent")
            .join("skills");

        Self {
            skills: Arc::new(Mutex::new(HashMap::new())),
            skills_dir,
        }
    }

    /// Create registry with custom skills directory
    pub fn with_dir(dir: PathBuf) -> Self {
        Self {
            skills: Arc::new(Mutex::new(HashMap::new())),
            skills_dir: dir,
        }
    }

    /// Register a skill
    pub fn register(&self, skill: Skill) -> Result<()> {
        let id = skill.meta.id.clone();
        let name = skill.meta.name.clone();

        let mut skills = self.skills.lock().unwrap();
        skills.insert(id.clone(), skill);

        info!("Registered skill: {} ({})", name, id);
        Ok(())
    }

    /// Unregister a skill
    pub fn unregister(&self, id: &str) -> Result<()> {
        let mut skills = self.skills.lock().unwrap();

        if skills.remove(id).is_some() {
            info!("Unregistered skill: {}", id);
            Ok(())
        } else {
            bail!("Skill not found: {}", id)
        }
    }

    /// Get a skill by ID
    pub fn get(&self, id: &str) -> Option<Skill> {
        let skills = self.skills.lock().unwrap();
        skills.get(id).map(|s| Skill {
            meta: s.meta.clone(),
            executor: s.executor.clone(),
        })
    }

    /// List all registered skills
    pub fn list(&self) -> Vec<SkillMeta> {
        let skills = self.skills.lock().unwrap();
        skills.values().map(|s| s.meta.clone()).collect()
    }

    /// Find skills by category
    pub fn by_category(&self, category: SkillCategory) -> Vec<SkillMeta> {
        let skills = self.skills.lock().unwrap();
        skills
            .values()
            .filter(|s| s.meta.category == category)
            .map(|s| s.meta.clone())
            .collect()
    }

    /// Search skills by name or tag
    pub fn search(&self, query: &str) -> Vec<SkillMeta> {
        let query_lower = query.to_lowercase();
        let skills = self.skills.lock().unwrap();

        skills
            .values()
            .filter(|s| {
                s.meta.name.to_lowercase().contains(&query_lower)
                    || s.meta.description.to_lowercase().contains(&query_lower)
                    || s.meta.tags.iter().any(|t| t.to_lowercase().contains(&query_lower))
            })
            .map(|s| s.meta.clone())
            .collect()
    }

    /// Execute a skill by ID
    pub fn execute(&self, id: &str, params: HashMap<String, String>, ctx: &SkillContext) -> Result<SkillResult> {
        let skill = {
            let skills = self.skills.lock().unwrap();
            skills.get(id).map(|s| Skill {
                meta: s.meta.clone(),
                executor: s.executor.clone(),
            })
        };

        if let Some(skill) = skill {
            // Validate required parameters
            for param in &skill.meta.parameters {
                if param.required && !params.contains_key(&param.name) {
                    if param.default.is_none() {
                        bail!("Missing required parameter: {}", param.name);
                    }
                }
            }

            let start = std::time::Instant::now();
            let result = skill.execute(params, ctx);
            let duration_ms = start.elapsed().as_millis() as u64;

            match result {
                Ok(mut result) => {
                    result.duration_ms = duration_ms;
                    Ok(result)
                }
                Err(e) => Ok(SkillResult {
                    success: false,
                    output: String::new(),
                    error: Some(e.to_string()),
                    duration_ms,
                }),
            }
        } else {
            bail!("Skill not found: {}", id)
        }
    }

    /// Save skill metadata to disk
    pub fn save_skill(&self, meta: &SkillMeta) -> Result<()> {
        std::fs::create_dir_all(&self.skills_dir)?;

        let path = self.skills_dir.join(format!("{}.json", meta.id));
        let content = serde_json::to_string_pretty(meta)?;
        std::fs::write(&path, content)?;

        info!("Saved skill metadata: {}", meta.id);
        Ok(())
    }

    /// Load skill metadata from disk
    pub fn load_skill(&self, id: &str) -> Result<SkillMeta> {
        let path = self.skills_dir.join(format!("{}.json", id));

        if !path.exists() {
            bail!("Skill file not found: {}", path.display());
        }

        let content = std::fs::read_to_string(&path)?;
        let meta: SkillMeta = serde_json::from_str(&content)?;

        Ok(meta)
    }

    /// Delete skill from disk
    pub fn delete_skill(&self, id: &str) -> Result<()> {
        let path = self.skills_dir.join(format!("{}.json", id));

        if path.exists() {
            std::fs::remove_file(&path)?;
            info!("Deleted skill file: {}", id);
        }

        Ok(())
    }

    /// List available skill files on disk
    pub fn list_saved_skills(&self) -> Result<Vec<String>> {
        if !self.skills_dir.exists() {
            return Ok(Vec::new());
        }

        let mut skill_ids = Vec::new();
        for entry in std::fs::read_dir(&self.skills_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Some(stem) = path.file_stem() {
                    skill_ids.push(stem.to_string_lossy().to_string());
                }
            }
        }

        Ok(skill_ids)
    }

    /// Get skills directory
    pub fn skills_dir(&self) -> &Path {
        &self.skills_dir
    }
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_meta_creation() {
        let meta = SkillMeta {
            id: "test-skill".to_string(),
            name: "Test Skill".to_string(),
            description: "A test skill".to_string(),
            version: "1.0.0".to_string(),
            author: Some("Test Author".to_string()),
            category: SkillCategory::Utility,
            permissions: vec![Permission::ReadFiles],
            parameters: vec![SkillParameter {
                name: "path".to_string(),
                param_type: ParameterType::Path,
                required: true,
                default: None,
                description: "File path".to_string(),
                allowed_values: None,
            }],
            builtin: false,
            tags: vec!["test".to_string()],
        };

        assert_eq!(meta.id, "test-skill");
        assert_eq!(meta.permissions.len(), 1);
    }

    #[test]
    fn test_registry_register() {
        let registry = SkillRegistry::new();

        let meta = SkillMeta {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: "Test skill".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            category: SkillCategory::Utility,
            permissions: vec![],
            parameters: vec![],
            builtin: false,
            tags: vec![],
        };

        let skill = Skill::new(meta, |_params, _ctx| {
            Ok(SkillResult {
                success: true,
                output: "done".to_string(),
                error: None,
                duration_ms: 0,
            })
        });

        registry.register(skill).unwrap();
        assert!(registry.get("test").is_some());
    }

    #[test]
    fn test_registry_search() {
        let registry = SkillRegistry::new();

        let meta = SkillMeta {
            id: "file-reader".to_string(),
            name: "File Reader".to_string(),
            description: "Reads files from disk".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            category: SkillCategory::Filesystem,
            permissions: vec![Permission::ReadFiles],
            parameters: vec![],
            builtin: true,
            tags: vec!["file".to_string(), "read".to_string()],
        };

        let skill = Skill::new(meta, |_params, _ctx| {
            Ok(SkillResult {
                success: true,
                output: String::new(),
                error: None,
                duration_ms: 0,
            })
        });

        registry.register(skill).unwrap();

        let results = registry.search("file");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "file-reader");
    }

    #[test]
    fn test_skill_execution() {
        let registry = SkillRegistry::new();

        let meta = SkillMeta {
            id: "echo".to_string(),
            name: "Echo".to_string(),
            description: "Echo input".to_string(),
            version: "1.0.0".to_string(),
            author: None,
            category: SkillCategory::Utility,
            permissions: vec![],
            parameters: vec![],
            builtin: true,
            tags: vec![],
        };

        let skill = Skill::new(meta, |params, _ctx| {
            let msg = params.get("message").cloned().unwrap_or_default();
            Ok(SkillResult {
                success: true,
                output: msg,
                error: None,
                duration_ms: 0,
            })
        });

        registry.register(skill).unwrap();

        let mut params = HashMap::new();
        params.insert("message".to_string(), "Hello, World!".to_string());

        let ctx = SkillContext::default();
        let result = registry.execute("echo", params, &ctx).unwrap();

        assert!(result.success);
        assert_eq!(result.output, "Hello, World!");
    }
}
