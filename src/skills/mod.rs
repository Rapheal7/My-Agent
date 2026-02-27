//! Dynamic skills module

pub mod registry;
pub mod loader;
pub mod generator;
pub mod executor;
pub mod builtin;
pub mod markdown;

use anyhow::Result;
use registry::{SkillRegistry, SkillMeta, Skill, SkillResult, SkillParameter};
use std::sync::OnceLock;
use tracing::warn;

/// Global skill registry singleton
static REGISTRY: OnceLock<SkillRegistry> = OnceLock::new();

/// Get the default skill registry with all built-in skills registered
pub fn default_registry() -> &'static SkillRegistry {
    REGISTRY.get_or_init(|| {
        let registry = SkillRegistry::new();

        // Register built-in skills
        let _ = registry.register(builtin::filesystem::create_skill());
        let _ = registry.register(builtin::shell::create_skill());
        let _ = registry.register(builtin::web::create_skill());
        let _ = registry.register(builtin::web_browsing::create_skill());
        let _ = registry.register(builtin::database::create_skill());

        // Load and register markdown skills from disk
        let md_skills = markdown::load_markdown_skills();
        for md_skill in md_skills {
            let skill = markdown_skill_to_registry_skill(md_skill);
            if let Err(e) = registry.register(skill) {
                warn!("Failed to register markdown skill: {}", e);
            }
        }

        registry
    })
}

/// Convert a MarkdownSkill into a registry Skill
pub fn markdown_skill_to_registry_skill(md_skill: markdown::MarkdownSkill) -> Skill {
    let fm = &md_skill.frontmatter;

    // Map frontmatter to SkillMeta
    let category = fm.category.as_deref()
        .map(markdown::parse_category)
        .unwrap_or(registry::SkillCategory::Custom);

    let permissions: Vec<registry::Permission> = fm.requires.as_ref()
        .map(|r| r.permissions.iter()
            .filter_map(|p| markdown::parse_permission(p))
            .collect())
        .unwrap_or_default();

    let parameters: Vec<SkillParameter> = fm.parameters.iter().map(|p| {
        SkillParameter {
            name: p.name.clone(),
            param_type: markdown::parse_parameter_type(&p.param_type),
            required: p.required,
            default: p.default.clone(),
            description: p.description.clone(),
            allowed_values: None,
        }
    }).collect();

    let meta = SkillMeta {
        id: md_skill.id.clone(),
        name: fm.name.clone(),
        description: fm.description.clone(),
        version: fm.version.clone().unwrap_or_else(|| "1.0.0".to_string()),
        author: fm.author.clone(),
        category,
        permissions,
        parameters,
        builtin: false,
        tags: fm.tags.clone(),
    };

    // The executor returns the instruction body — the LLM reads and follows it
    let body = md_skill.body.clone();
    Skill::new(meta, move |_params, _ctx| {
        Ok(SkillResult {
            success: true,
            output: body.clone(),
            error: None,
            duration_ms: 0,
        })
    })
}

/// List installed skills
pub fn list_skills() -> Result<()> {
    let registry = default_registry();
    let skills = registry.list();

    // Also load markdown skills separately to show type info
    let md_skills = markdown::load_markdown_skills();
    let md_ids: std::collections::HashSet<String> = md_skills.iter().map(|s| s.id.clone()).collect();

    println!("Installed skills ({}):\n", skills.len());

    for skill in &skills {
        let type_marker = if skill.builtin {
            "[built-in]"
        } else if md_ids.contains(&skill.id) {
            "[markdown]"
        } else {
            "[rhai]"
        };
        println!("  {} {} - {}", skill.name, type_marker, skill.description);
        println!("    ID: {}", skill.id);
        println!("    Category: {:?}", skill.category);
        println!("    Permissions: {:?}", skill.permissions);
        println!("    Tags: {}", skill.tags.join(", "));
        println!();
    }

    Ok(())
}

/// Install a new skill
pub async fn install_skill(_name: &str) -> Result<()> {
    println!("Skill installation not yet implemented.");
    Ok(())
}

/// Remove a skill by name/id — deletes the .skill.md or .json file
pub fn remove_skill(name: &str) -> Result<()> {
    let skills_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("my-agent")
        .join("skills");

    // Try .skill.md first
    let md_path = skills_dir.join(format!("{}.skill.md", name));
    if md_path.exists() {
        std::fs::remove_file(&md_path)?;
        println!("Removed skill file: {}", md_path.display());

        // Unregister from active registry if loaded
        let registry = default_registry();
        let _ = registry.unregister(name);
        return Ok(());
    }

    // Try .json (Rhai skill metadata)
    let json_path = skills_dir.join(format!("{}.json", name));
    if json_path.exists() {
        std::fs::remove_file(&json_path)?;
        println!("Removed skill file: {}", json_path.display());

        let registry = default_registry();
        let _ = registry.unregister(name);
        return Ok(());
    }

    println!("Skill '{}' not found in {}", name, skills_dir.display());
    Ok(())
}
