//! Dynamic skills module

pub mod registry;
pub mod loader;
pub mod generator;
pub mod executor;
pub mod builtin;

use anyhow::Result;
use registry::{SkillRegistry, SkillMeta};
use std::sync::OnceLock;

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

        registry
    })
}

/// List installed skills
pub fn list_skills() -> Result<()> {
    let registry = default_registry();
    let skills = registry.list();

    println!("Installed skills ({}):\n", skills.len());

    for skill in &skills {
        let builtin_marker = if skill.builtin { "[built-in]" } else { "" };
        println!("  {} {} - {}", skill.name, builtin_marker, skill.description);
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

/// Remove a skill
pub fn remove_skill(_name: &str) -> Result<()> {
    println!("Skill removal not yet implemented.");
    Ok(())
}
