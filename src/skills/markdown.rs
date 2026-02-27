//! SKILL.md format parser and types
//!
//! Skills defined as natural language instruction documents (OpenClaw-style).
//! The LLM reads the instructions on demand and follows them.
//!
//! Format:
//! ```markdown
//! ---
//! name: My Skill
//! description: What the skill does
//! version: 1.0.0
//! tags: [tag1, tag2]
//! ---
//! # Instructions
//! Natural language steps the LLM should follow...
//! ```

use anyhow::{Result, Context, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::warn;

/// SKILL.md frontmatter (YAML between --- delimiters)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// Display name
    pub name: String,
    /// What the skill does
    pub description: String,
    /// Semver version (default "1.0.0")
    #[serde(default = "default_version")]
    pub version: Option<String>,
    /// Author
    #[serde(default)]
    pub author: Option<String>,
    /// Tags for discovery/search
    #[serde(default)]
    pub tags: Vec<String>,
    /// Category — maps to SkillCategory
    #[serde(default)]
    pub category: Option<String>,
    /// Dependencies
    #[serde(default)]
    pub requires: Option<SkillRequirements>,
    /// Input parameter definitions
    #[serde(default)]
    pub parameters: Vec<SkillParamDef>,
}

fn default_version() -> Option<String> {
    Some("1.0.0".to_string())
}

/// Skill dependencies
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillRequirements {
    /// Required environment variables
    #[serde(default)]
    pub env: Vec<String>,
    /// Required binaries on PATH
    #[serde(default)]
    pub bins: Vec<String>,
    /// Required permissions (maps to Permission enum names)
    #[serde(default)]
    pub permissions: Vec<String>,
}

/// Skill parameter definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillParamDef {
    /// Parameter name
    pub name: String,
    /// Type as a string (String, Integer, Boolean, Path, etc.)
    #[serde(default = "default_param_type")]
    pub param_type: String,
    /// Whether the parameter is required
    #[serde(default)]
    pub required: bool,
    /// Default value
    #[serde(default)]
    pub default: Option<String>,
    /// Description of the parameter
    #[serde(default)]
    pub description: String,
}

fn default_param_type() -> String {
    "String".to_string()
}

/// A parsed SKILL.md document
#[derive(Debug, Clone)]
pub struct MarkdownSkill {
    /// ID derived from filename (e.g., deploy-app.skill.md → "deploy-app")
    pub id: String,
    /// Parsed frontmatter
    pub frontmatter: SkillFrontmatter,
    /// Natural language instruction body (everything after second ---)
    pub body: String,
    /// Source file path
    pub file_path: PathBuf,
}

/// Parse a SKILL.md file into a MarkdownSkill
pub fn parse_skill_md(path: &Path) -> Result<MarkdownSkill> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read skill file: {}", path.display()))?;

    // Split on --- delimiters
    let trimmed = content.trim();
    if !trimmed.starts_with("---") {
        bail!("SKILL.md must start with '---' frontmatter delimiter");
    }

    // Find the second ---
    let after_first = &trimmed[3..];
    let second_delim = after_first.find("\n---")
        .ok_or_else(|| anyhow::anyhow!("Missing closing '---' frontmatter delimiter"))?;

    let yaml_str = after_first[..second_delim].trim();
    let body = after_first[second_delim + 4..].trim().to_string();

    // Parse YAML frontmatter
    let frontmatter: SkillFrontmatter = serde_yaml::from_str(yaml_str)
        .with_context(|| format!("Failed to parse frontmatter YAML in {}", path.display()))?;

    // Derive ID from filename
    let stem = path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    // Strip .skill suffix if present (e.g., "deploy-app.skill.md" → stem is "deploy-app.skill" → id "deploy-app")
    let id = stem.strip_suffix(".skill").unwrap_or(stem).to_string();

    Ok(MarkdownSkill {
        id,
        frontmatter,
        body,
        file_path: path.to_path_buf(),
    })
}

/// Check if a skill's requirements are met. Returns a list of unmet requirements.
pub fn check_requirements(skill: &MarkdownSkill) -> Vec<String> {
    let mut missing = Vec::new();

    if let Some(ref requires) = skill.frontmatter.requires {
        // Check environment variables
        for env_var in &requires.env {
            if std::env::var(env_var).is_err() {
                missing.push(format!("env var '{}' not set", env_var));
            }
        }

        // Check binaries on PATH
        for bin in &requires.bins {
            let found = std::process::Command::new("which")
                .arg(bin)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !found {
                missing.push(format!("binary '{}' not found on PATH", bin));
            }
        }
    }

    missing
}

/// Generate a template SKILL.md content for scaffolding
pub fn generate_template(name: &str, description: &str) -> String {
    let id = name.to_lowercase().replace(' ', "-");
    format!(
        r#"---
name: {name}
description: {description}
version: 1.0.0
author: user
tags: []
category: Utility
parameters: []
---
# {name}

## Steps

1. Describe the first step here...
2. Describe the second step here...
3. Verify the result
"#,
        name = name,
        description = description,
    )
}

/// Map a category string to a SkillCategory
pub fn parse_category(s: &str) -> super::registry::SkillCategory {
    match s.to_lowercase().as_str() {
        "filesystem" | "file" => super::registry::SkillCategory::Filesystem,
        "shell" | "command" => super::registry::SkillCategory::Shell,
        "web" | "http" | "api" => super::registry::SkillCategory::Web,
        "data" | "processing" => super::registry::SkillCategory::Data,
        "system" => super::registry::SkillCategory::System,
        "utility" | "util" => super::registry::SkillCategory::Utility,
        _ => super::registry::SkillCategory::Custom,
    }
}

/// Map a permission string to a Permission enum
pub fn parse_permission(s: &str) -> Option<super::registry::Permission> {
    match s.to_lowercase().as_str() {
        "readfiles" | "read_files" => Some(super::registry::Permission::ReadFiles),
        "writefiles" | "write_files" => Some(super::registry::Permission::WriteFiles),
        "executecommands" | "execute_commands" => Some(super::registry::Permission::ExecuteCommands),
        "networkaccess" | "network_access" => Some(super::registry::Permission::NetworkAccess),
        "readenvironment" | "read_environment" => Some(super::registry::Permission::ReadEnvironment),
        "systemmodify" | "system_modify" => Some(super::registry::Permission::SystemModify),
        _ => None,
    }
}

/// Map a param_type string to a ParameterType
pub fn parse_parameter_type(s: &str) -> super::registry::ParameterType {
    match s.to_lowercase().as_str() {
        "string" | "str" => super::registry::ParameterType::String,
        "integer" | "int" => super::registry::ParameterType::Integer,
        "float" | "number" => super::registry::ParameterType::Float,
        "boolean" | "bool" => super::registry::ParameterType::Boolean,
        "path" | "file" => super::registry::ParameterType::Path,
        "url" => super::registry::ParameterType::Url,
        "enum" => super::registry::ParameterType::Enum,
        "array" | "list" => super::registry::ParameterType::Array,
        "object" | "map" => super::registry::ParameterType::Object,
        _ => super::registry::ParameterType::String,
    }
}

/// Load all SKILL.md files from the skills directory
pub fn load_markdown_skills() -> Vec<MarkdownSkill> {
    let skills_dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("my-agent")
        .join("skills");

    load_markdown_skills_from(&skills_dir)
}

/// Load SKILL.md files from a specific directory
pub fn load_markdown_skills_from(dir: &Path) -> Vec<MarkdownSkill> {
    let mut skills = Vec::new();

    if !dir.exists() {
        return skills;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!("Failed to read skills directory {}: {}", dir.display(), e);
            return skills;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if name.ends_with(".skill.md") || (name.ends_with(".skill") && !path.is_dir()) {
            match parse_skill_md(&path) {
                Ok(skill) => {
                    skills.push(skill);
                }
                Err(e) => {
                    warn!("Skipping malformed skill file {}: {}", path.display(), e);
                }
            }
        }
    }

    skills
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_skill_md() {
        let content = r#"---
name: Test Skill
description: A test skill
version: 1.0.0
tags: [test, example]
category: Utility
parameters:
  - name: target
    param_type: String
    required: true
    description: Target to test
---
# Test Skill

## Steps

1. Do something
2. Verify result
"#;
        let mut file = NamedTempFile::with_suffix(".skill.md").unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let skill = parse_skill_md(file.path()).unwrap();
        assert_eq!(skill.frontmatter.name, "Test Skill");
        assert_eq!(skill.frontmatter.description, "A test skill");
        assert_eq!(skill.frontmatter.tags, vec!["test", "example"]);
        assert_eq!(skill.frontmatter.parameters.len(), 1);
        assert_eq!(skill.frontmatter.parameters[0].name, "target");
        assert!(skill.body.contains("Do something"));
    }

    #[test]
    fn test_parse_skill_md_missing_delimiter() {
        let content = "---\nname: Bad Skill\n";
        let mut file = NamedTempFile::with_suffix(".skill.md").unwrap();
        file.write_all(content.as_bytes()).unwrap();

        assert!(parse_skill_md(file.path()).is_err());
    }

    #[test]
    fn test_check_requirements_met() {
        let skill = MarkdownSkill {
            id: "test".to_string(),
            frontmatter: SkillFrontmatter {
                name: "Test".to_string(),
                description: "Test".to_string(),
                version: Some("1.0.0".to_string()),
                author: None,
                tags: vec![],
                category: None,
                requires: None,
                parameters: vec![],
            },
            body: String::new(),
            file_path: PathBuf::new(),
        };
        assert!(check_requirements(&skill).is_empty());
    }

    #[test]
    fn test_check_requirements_missing_bin() {
        let skill = MarkdownSkill {
            id: "test".to_string(),
            frontmatter: SkillFrontmatter {
                name: "Test".to_string(),
                description: "Test".to_string(),
                version: Some("1.0.0".to_string()),
                author: None,
                tags: vec![],
                category: None,
                requires: Some(SkillRequirements {
                    env: vec![],
                    bins: vec!["nonexistent_binary_xyz_123".to_string()],
                    permissions: vec![],
                }),
                parameters: vec![],
            },
            body: String::new(),
            file_path: PathBuf::new(),
        };
        let missing = check_requirements(&skill);
        assert_eq!(missing.len(), 1);
        assert!(missing[0].contains("nonexistent_binary_xyz_123"));
    }

    #[test]
    fn test_generate_template() {
        let template = generate_template("Deploy App", "Deploy the application");
        assert!(template.contains("name: Deploy App"));
        assert!(template.contains("description: Deploy the application"));
        assert!(template.contains("version: 1.0.0"));
    }

    #[test]
    fn test_parse_category() {
        assert!(matches!(parse_category("Shell"), super::super::registry::SkillCategory::Shell));
        assert!(matches!(parse_category("web"), super::super::registry::SkillCategory::Web));
        assert!(matches!(parse_category("unknown"), super::super::registry::SkillCategory::Custom));
    }
}
