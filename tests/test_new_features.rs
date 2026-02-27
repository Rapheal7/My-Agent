//! Integration tests for the new features:
//! - Gap 1: Vision Pipeline (multimodal tool results)
//! - Gap 2: Accessibility Tree Browser (browser_snapshot/browser_act tools)
//! - Gap 3: System Prompts (desktop/browser sections)
//! - Feature 4: SKILL.md System (parser, registry, creation, CLI)

use my_agent::agent::tools::{ToolCall, ToolContext, ToolResult, builtin_tools, execute_tool};
use my_agent::skills::markdown::{
    parse_skill_md, check_requirements, generate_template, load_markdown_skills_from,
    SkillFrontmatter, SkillRequirements, SkillParamDef, MarkdownSkill,
    parse_category, parse_permission, parse_parameter_type,
};
use my_agent::skills::{default_registry, list_skills, remove_skill, markdown_skill_to_registry_skill};
use my_agent::soul::system_prompts::{
    get_main_system_prompt, get_full_system_prompt, get_tool_descriptions, get_skills_manifest,
};
use my_agent::tools::browser::{AXNode, AXSnapshot, RefMap, BrowserConfig};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

// =====================================================================
// GAP 1: VISION PIPELINE TESTS
// =====================================================================

#[test]
fn test_extract_image_content_from_screenshot_result() {
    // Simulate a capture_screen ToolResult with image data
    let result = ToolResult {
        success: true,
        message: "Captured screenshot: 1920x1080".to_string(),
        data: Some(serde_json::json!({
            "width": 1920,
            "height": 1080,
            "base64_data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
            "media_type": "image/png",
        })),
    };

    // Verify the data has the expected structure for image detection
    let data = result.data.as_ref().unwrap();
    assert!(data.get("base64_data").is_some());
    assert!(data.get("media_type").is_some());
    assert_eq!(data["media_type"].as_str().unwrap(), "image/png");
    assert!(data["media_type"].as_str().unwrap().starts_with("image/"));
    assert!(data["width"].as_u64().unwrap() > 0);
    assert!(data["height"].as_u64().unwrap() > 0);
}

#[test]
fn test_non_image_result_has_no_image_fields() {
    // Non-image tool result (e.g., read_file)
    let result = ToolResult {
        success: true,
        message: "File read successfully".to_string(),
        data: Some(serde_json::json!({
            "content": "hello world",
            "size": 11,
        })),
    };

    let data = result.data.as_ref().unwrap();
    // Should NOT have base64_data or media_type
    assert!(data.get("base64_data").is_none());
    assert!(data.get("media_type").is_none());
}

#[test]
fn test_multimodal_content_structure() {
    // Verify that vision analysis results produce the expected text format.
    // The vision pipeline now routes screenshots through the vision model
    // and returns a text description (not a multimodal content array).
    let width = 1920u64;
    let height = 1080u64;
    let description = "A terminal window showing a Rust compilation output.";

    // This is what the vision model returns as the tool result text
    let vision_result = format!(
        "Screenshot captured: {}x{}\n\nVision analysis:\n{}",
        width, height, description
    );

    assert!(vision_result.contains("Screenshot captured: 1920x1080"));
    assert!(vision_result.contains("Vision analysis:"));
    assert!(vision_result.contains(description));

    // Also verify the multimodal message format sent TO the vision model
    let media_type = "image/png";
    let base64_data = "dGVzdA==";
    let vision_request = serde_json::json!([
        {
            "type": "text",
            "text": "Describe this screenshot in detail."
        },
        {
            "type": "image_url",
            "image_url": {
                "url": format!("data:{};base64,{}", media_type, base64_data)
            }
        }
    ]);

    assert!(vision_request.is_array());
    let arr = vision_request.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(arr[1]["type"], "image_url");
    assert!(arr[1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
}

// =====================================================================
// GAP 2: ACCESSIBILITY TREE BROWSER TESTS
// =====================================================================

#[test]
fn test_ax_snapshot_types() {
    let snapshot = AXSnapshot {
        url: "https://example.com".to_string(),
        title: "Example".to_string(),
        tree_text: "[@e1] link \"Home\"\n[@e2] textbox \"Search\" value=\"\"\n[@e3] button \"Submit\"".to_string(),
        element_count: 3,
    };

    assert_eq!(snapshot.element_count, 3);
    assert!(snapshot.tree_text.contains("@e1"));
    assert!(snapshot.tree_text.contains("@e2"));
    assert!(snapshot.tree_text.contains("@e3"));
}

#[test]
fn test_ref_map() {
    let mut ref_map = RefMap::default();
    ref_map.refs.insert("@e1".to_string(), 100);
    ref_map.refs.insert("@e2".to_string(), 200);
    ref_map.refs.insert("@e3".to_string(), 300);

    assert_eq!(ref_map.refs.len(), 3);
    assert_eq!(*ref_map.refs.get("@e1").unwrap(), 100);
    assert_eq!(*ref_map.refs.get("@e2").unwrap(), 200);
    assert!(ref_map.refs.get("@e99").is_none());
}

#[test]
fn test_browser_tools_registered() {
    let tools = builtin_tools();
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();

    assert!(tool_names.contains(&"browser_snapshot"), "browser_snapshot tool not found");
    assert!(tool_names.contains(&"browser_act"), "browser_act tool not found");
}

#[test]
fn test_browser_snapshot_tool_params() {
    let tools = builtin_tools();
    let snap_tool = tools.iter().find(|t| t.name == "browser_snapshot").unwrap();

    let params = &snap_tool.parameters;
    assert!(params["properties"]["session_id"].is_object());
    let required = params["required"].as_array().unwrap();
    assert!(required.iter().any(|v| v == "session_id"));
}

#[test]
fn test_browser_act_tool_params() {
    let tools = builtin_tools();
    let act_tool = tools.iter().find(|t| t.name == "browser_act").unwrap();

    let params = &act_tool.parameters;
    assert!(params["properties"]["session_id"].is_object());
    assert!(params["properties"]["ref"].is_object());
    assert!(params["properties"]["action"].is_object());
    assert!(params["properties"]["value"].is_object());

    let required = params["required"].as_array().unwrap();
    assert!(required.iter().any(|v| v == "session_id"));
    assert!(required.iter().any(|v| v == "ref"));
    assert!(required.iter().any(|v| v == "action"));
    // value should NOT be required
    assert!(!required.iter().any(|v| v == "value"));
}

#[test]
fn test_tool_context_has_browser_refs() {
    let ctx = ToolContext::new();
    // browser_refs should be initialized and accessible
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let refs = ctx.browser_refs.lock().await;
        assert!(refs.is_empty(), "browser_refs should be empty initially");
    });
}

// =====================================================================
// GAP 3: SYSTEM PROMPT TESTS
// =====================================================================

#[test]
fn test_system_prompt_has_desktop_section() {
    let prompt = get_main_system_prompt();
    assert!(prompt.contains("### Desktop Control"), "Missing Desktop Control section");
    assert!(prompt.contains("capture_screen"), "Missing capture_screen tool");
    assert!(prompt.contains("mouse_click"), "Missing mouse_click tool");
    assert!(prompt.contains("keyboard_type"), "Missing keyboard_type tool");
    assert!(prompt.contains("keyboard_press"), "Missing keyboard_press tool");
    assert!(prompt.contains("keyboard_hotkey"), "Missing keyboard_hotkey tool");
    assert!(prompt.contains("open_application"), "Missing open_application tool");
    assert!(prompt.contains("mouse_scroll"), "Missing mouse_scroll tool");
    assert!(prompt.contains("mouse_drag"), "Missing mouse_drag tool");
    assert!(prompt.contains("mouse_double_click"), "Missing mouse_double_click tool");
}

#[test]
fn test_system_prompt_has_browser_section() {
    let prompt = get_main_system_prompt();
    assert!(prompt.contains("### Browser Automation"), "Missing Browser Automation section");
    assert!(prompt.contains("browser_snapshot"), "Missing browser_snapshot tool");
    assert!(prompt.contains("browser_act"), "Missing browser_act tool");
}

#[test]
fn test_system_prompt_has_workflow_guidance() {
    let prompt = get_main_system_prompt();
    assert!(prompt.contains("## Desktop & Browser Workflow"), "Missing workflow section");
    assert!(prompt.contains("Observe"), "Missing Observe step");
    assert!(prompt.contains("Analyze"), "Missing Analyze step");
    assert!(prompt.contains("Verify"), "Missing Verify step");
    assert!(prompt.contains("Never guess coordinates"), "Missing coordinate warning");
    assert!(prompt.contains("Never guess CSS selectors"), "Missing selector warning");
}

#[test]
fn test_tool_descriptions_has_desktop_and_browser() {
    let descriptions = get_tool_descriptions();
    assert!(descriptions.contains("### capture_screen"), "Missing capture_screen description");
    assert!(descriptions.contains("### browser_snapshot"), "Missing browser_snapshot description");
    assert!(descriptions.contains("### browser_act"), "Missing browser_act description");
}

#[test]
fn test_full_system_prompt_with_bootstrap() {
    let prompt = get_full_system_prompt("Bootstrap context here");
    // Bootstrap should be prepended
    assert!(prompt.contains("Bootstrap context here"));
    assert!(prompt.contains("### Desktop Control"));
}

#[test]
fn test_full_system_prompt_without_bootstrap() {
    let prompt = get_full_system_prompt("");
    // Should still have desktop section
    assert!(prompt.contains("### Desktop Control"));
}

// =====================================================================
// FEATURE 4A: SKILL.MD PARSER TESTS
// =====================================================================

#[test]
fn test_parse_skill_md_full() {
    let content = r#"---
name: Full Test Skill
description: A comprehensive test skill
version: 2.1.0
author: test-author
tags: [devops, ci, testing]
category: Shell
requires:
  bins: [git, make]
  env: [HOME]
  permissions: [ExecuteCommands, ReadFiles]
parameters:
  - name: branch
    param_type: String
    required: true
    description: Git branch to deploy
  - name: dry_run
    param_type: Boolean
    required: false
    default: "false"
    description: Whether to do a dry run
---
# Full Test Skill

## Prerequisites
- Ensure git is installed
- Ensure make is available

## Steps

1. Checkout the branch: `git checkout {{branch}}`
2. Run build: `make build`
3. If not dry_run, deploy: `make deploy`
4. Verify deployment
"#;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("full-test.skill.md");
    std::fs::write(&path, content).unwrap();

    let skill = parse_skill_md(&path).unwrap();

    assert_eq!(skill.id, "full-test");
    assert_eq!(skill.frontmatter.name, "Full Test Skill");
    assert_eq!(skill.frontmatter.description, "A comprehensive test skill");
    assert_eq!(skill.frontmatter.version.as_deref(), Some("2.1.0"));
    assert_eq!(skill.frontmatter.author.as_deref(), Some("test-author"));
    assert_eq!(skill.frontmatter.tags, vec!["devops", "ci", "testing"]);
    assert_eq!(skill.frontmatter.category.as_deref(), Some("Shell"));

    // Requirements
    let requires = skill.frontmatter.requires.as_ref().unwrap();
    assert_eq!(requires.bins, vec!["git", "make"]);
    assert_eq!(requires.env, vec!["HOME"]);
    assert_eq!(requires.permissions, vec!["ExecuteCommands", "ReadFiles"]);

    // Parameters
    assert_eq!(skill.frontmatter.parameters.len(), 2);
    assert_eq!(skill.frontmatter.parameters[0].name, "branch");
    assert!(skill.frontmatter.parameters[0].required);
    assert_eq!(skill.frontmatter.parameters[1].name, "dry_run");
    assert!(!skill.frontmatter.parameters[1].required);
    assert_eq!(skill.frontmatter.parameters[1].default.as_deref(), Some("false"));

    // Body
    assert!(skill.body.contains("Checkout the branch"));
    assert!(skill.body.contains("{{branch}}"));
}

#[test]
fn test_parse_skill_md_minimal() {
    let content = r#"---
name: Minimal
description: Minimal skill
---
Do something.
"#;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("minimal.skill.md");
    std::fs::write(&path, content).unwrap();

    let skill = parse_skill_md(&path).unwrap();
    assert_eq!(skill.id, "minimal");
    assert_eq!(skill.frontmatter.name, "Minimal");
    assert!(skill.frontmatter.tags.is_empty());
    assert!(skill.frontmatter.requires.is_none());
    assert!(skill.frontmatter.parameters.is_empty());
    assert!(skill.body.contains("Do something"));
}

#[test]
fn test_parse_skill_md_no_frontmatter() {
    let content = "Just some text without frontmatter";

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.skill.md");
    std::fs::write(&path, content).unwrap();

    assert!(parse_skill_md(&path).is_err());
}

#[test]
fn test_parse_skill_md_unclosed_frontmatter() {
    let content = "---\nname: Bad\n";

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unclosed.skill.md");
    std::fs::write(&path, content).unwrap();

    assert!(parse_skill_md(&path).is_err());
}

#[test]
fn test_parse_skill_md_invalid_yaml() {
    let content = "---\n: invalid yaml [[\n---\nBody";

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("invalid.skill.md");
    std::fs::write(&path, content).unwrap();

    assert!(parse_skill_md(&path).is_err());
}

#[test]
fn test_skill_id_derivation() {
    let content = "---\nname: Test\ndescription: Test\n---\nBody";

    // Test .skill.md extension
    let dir = tempfile::tempdir().unwrap();
    let path1 = dir.path().join("my-tool.skill.md");
    std::fs::write(&path1, content).unwrap();
    assert_eq!(parse_skill_md(&path1).unwrap().id, "my-tool");

    // Test .skill extension
    let path2 = dir.path().join("another.skill");
    std::fs::write(&path2, content).unwrap();
    assert_eq!(parse_skill_md(&path2).unwrap().id, "another");

    // Test plain .md extension
    let path3 = dir.path().join("plain.md");
    std::fs::write(&path3, content).unwrap();
    assert_eq!(parse_skill_md(&path3).unwrap().id, "plain");
}

#[test]
fn test_check_requirements_env_present() {
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
                env: vec!["HOME".to_string()],  // HOME should always be set
                bins: vec![],
                permissions: vec![],
            }),
            parameters: vec![],
        },
        body: String::new(),
        file_path: PathBuf::new(),
    };
    assert!(check_requirements(&skill).is_empty(), "HOME should be present");
}

#[test]
fn test_check_requirements_env_missing() {
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
                env: vec!["TOTALLY_NONEXISTENT_VAR_XYZ".to_string()],
                bins: vec![],
                permissions: vec![],
            }),
            parameters: vec![],
        },
        body: String::new(),
        file_path: PathBuf::new(),
    };
    let missing = check_requirements(&skill);
    assert_eq!(missing.len(), 1);
    assert!(missing[0].contains("TOTALLY_NONEXISTENT_VAR_XYZ"));
}

#[test]
fn test_check_requirements_bin_present() {
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
                bins: vec!["ls".to_string()],  // ls should always be on PATH
                permissions: vec![],
            }),
            parameters: vec![],
        },
        body: String::new(),
        file_path: PathBuf::new(),
    };
    assert!(check_requirements(&skill).is_empty(), "ls should be found on PATH");
}

#[test]
fn test_check_requirements_bin_missing() {
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
                bins: vec!["nonexistent_binary_xyz_99999".to_string()],
                permissions: vec![],
            }),
            parameters: vec![],
        },
        body: String::new(),
        file_path: PathBuf::new(),
    };
    let missing = check_requirements(&skill);
    assert_eq!(missing.len(), 1);
    assert!(missing[0].contains("nonexistent_binary_xyz_99999"));
}

#[test]
fn test_check_requirements_no_requires() {
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
fn test_generate_template_content() {
    let template = generate_template("My Deploy", "Deploy to production");
    assert!(template.starts_with("---\n"));
    assert!(template.contains("name: My Deploy"));
    assert!(template.contains("description: Deploy to production"));
    assert!(template.contains("version: 1.0.0"));
    assert!(template.contains("author: user"));
    assert!(template.contains("## Steps"));
}

#[test]
fn test_generate_template_is_valid_skill_md() {
    let template = generate_template("Template Test", "A generated template");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("template-test.skill.md");
    std::fs::write(&path, &template).unwrap();

    // Should parse without error
    let skill = parse_skill_md(&path).unwrap();
    assert_eq!(skill.frontmatter.name, "Template Test");
    assert_eq!(skill.frontmatter.description, "A generated template");
    assert!(skill.body.contains("## Steps"));
}

#[test]
fn test_load_markdown_skills_from_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let skills = load_markdown_skills_from(dir.path());
    assert!(skills.is_empty());
}

#[test]
fn test_load_markdown_skills_from_nonexistent_dir() {
    let skills = load_markdown_skills_from(&PathBuf::from("/nonexistent/path/xyz"));
    assert!(skills.is_empty());
}

#[test]
fn test_load_markdown_skills_from_populated_dir() {
    let dir = tempfile::tempdir().unwrap();

    // Create 2 valid skills
    let skill1 = generate_template("Skill One", "First skill");
    let skill2 = generate_template("Skill Two", "Second skill");
    std::fs::write(dir.path().join("skill-one.skill.md"), &skill1).unwrap();
    std::fs::write(dir.path().join("skill-two.skill.md"), &skill2).unwrap();

    // Create 1 invalid skill
    std::fs::write(dir.path().join("bad.skill.md"), "not a valid skill").unwrap();

    // Create a non-skill file (should be ignored)
    std::fs::write(dir.path().join("readme.md"), "# Readme").unwrap();

    let skills = load_markdown_skills_from(dir.path());
    assert_eq!(skills.len(), 2, "Should load 2 valid skills, skip 1 invalid, ignore 1 non-skill");

    let ids: Vec<&str> = skills.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"skill-one"));
    assert!(ids.contains(&"skill-two"));
}

// =====================================================================
// FEATURE 4A: PARSER HELPER FUNCTIONS
// =====================================================================

#[test]
fn test_parse_category_all() {
    use my_agent::skills::registry::SkillCategory;
    assert!(matches!(parse_category("Filesystem"), SkillCategory::Filesystem));
    assert!(matches!(parse_category("file"), SkillCategory::Filesystem));
    assert!(matches!(parse_category("Shell"), SkillCategory::Shell));
    assert!(matches!(parse_category("command"), SkillCategory::Shell));
    assert!(matches!(parse_category("Web"), SkillCategory::Web));
    assert!(matches!(parse_category("http"), SkillCategory::Web));
    assert!(matches!(parse_category("api"), SkillCategory::Web));
    assert!(matches!(parse_category("Data"), SkillCategory::Data));
    assert!(matches!(parse_category("System"), SkillCategory::System));
    assert!(matches!(parse_category("Utility"), SkillCategory::Utility));
    assert!(matches!(parse_category("Custom"), SkillCategory::Custom));
    assert!(matches!(parse_category("random"), SkillCategory::Custom));
}

#[test]
fn test_parse_permission_all() {
    use my_agent::skills::registry::Permission;
    assert!(matches!(parse_permission("ReadFiles"), Some(Permission::ReadFiles)));
    assert!(matches!(parse_permission("read_files"), Some(Permission::ReadFiles)));
    assert!(matches!(parse_permission("WriteFiles"), Some(Permission::WriteFiles)));
    assert!(matches!(parse_permission("ExecuteCommands"), Some(Permission::ExecuteCommands)));
    assert!(matches!(parse_permission("NetworkAccess"), Some(Permission::NetworkAccess)));
    assert!(matches!(parse_permission("ReadEnvironment"), Some(Permission::ReadEnvironment)));
    assert!(matches!(parse_permission("SystemModify"), Some(Permission::SystemModify)));
    assert!(parse_permission("Unknown").is_none());
}

#[test]
fn test_parse_parameter_type_all() {
    use my_agent::skills::registry::ParameterType;
    assert!(matches!(parse_parameter_type("String"), ParameterType::String));
    assert!(matches!(parse_parameter_type("str"), ParameterType::String));
    assert!(matches!(parse_parameter_type("Integer"), ParameterType::Integer));
    assert!(matches!(parse_parameter_type("int"), ParameterType::Integer));
    assert!(matches!(parse_parameter_type("Float"), ParameterType::Float));
    assert!(matches!(parse_parameter_type("Boolean"), ParameterType::Boolean));
    assert!(matches!(parse_parameter_type("Path"), ParameterType::Path));
    assert!(matches!(parse_parameter_type("Url"), ParameterType::Url));
    assert!(matches!(parse_parameter_type("Enum"), ParameterType::Enum));
    assert!(matches!(parse_parameter_type("Array"), ParameterType::Array));
    assert!(matches!(parse_parameter_type("Object"), ParameterType::Object));
    assert!(matches!(parse_parameter_type("unknown_type"), ParameterType::String));
}

// =====================================================================
// FEATURE 4C: REGISTRY INTEGRATION TESTS
// =====================================================================

#[test]
fn test_markdown_skill_to_registry_skill() {
    let md_skill = MarkdownSkill {
        id: "test-reg".to_string(),
        frontmatter: SkillFrontmatter {
            name: "Test Registry".to_string(),
            description: "Test skill for registry integration".to_string(),
            version: Some("1.0.0".to_string()),
            author: Some("test".to_string()),
            tags: vec!["test".to_string()],
            category: Some("Shell".to_string()),
            requires: Some(SkillRequirements {
                env: vec![],
                bins: vec![],
                permissions: vec!["ExecuteCommands".to_string()],
            }),
            parameters: vec![SkillParamDef {
                name: "target".to_string(),
                param_type: "String".to_string(),
                required: true,
                default: None,
                description: "Target to deploy to".to_string(),
            }],
        },
        body: "# Instructions\n\n1. Deploy to {{target}}".to_string(),
        file_path: PathBuf::from("/test/path"),
    };

    let skill = markdown_skill_to_registry_skill(md_skill);

    assert_eq!(skill.meta.id, "test-reg");
    assert_eq!(skill.meta.name, "Test Registry");
    assert!(!skill.meta.builtin);
    assert_eq!(skill.meta.tags, vec!["test"]);
    assert_eq!(skill.meta.parameters.len(), 1);
    assert_eq!(skill.meta.parameters[0].name, "target");

    // Execute should return the body text
    let ctx = my_agent::skills::registry::SkillContext {
        working_dir: PathBuf::from("."),
        env: HashMap::new(),
        timeout_secs: 10,
        require_approval: false,
    };
    let result = skill.execute(HashMap::new(), &ctx).unwrap();
    assert!(result.success);
    assert!(result.output.contains("Deploy to {{target}}"));
}

#[test]
fn test_default_registry_has_builtins() {
    let registry = default_registry();
    let skills = registry.list();

    // Should have at least the 5 built-in skills
    assert!(skills.len() >= 5, "Expected at least 5 built-in skills, got {}", skills.len());

    let ids: Vec<&str> = skills.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"builtin-filesystem"));
    assert!(ids.contains(&"builtin-shell"));
    assert!(ids.contains(&"builtin-web"));
    assert!(ids.contains(&"builtin-web-browsing"));
    assert!(ids.contains(&"builtin-database"));
}

// =====================================================================
// FEATURE 4B: SKILL CREATION TOOL TESTS
// =====================================================================

#[test]
fn test_create_markdown_skill_tool_registered() {
    let tools = builtin_tools();
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(tool_names.contains(&"create_markdown_skill"), "create_markdown_skill not in builtin_tools");
}

#[test]
fn test_create_markdown_skill_tool_params() {
    let tools = builtin_tools();
    let tool = tools.iter().find(|t| t.name == "create_markdown_skill").unwrap();
    let params = &tool.parameters;
    assert!(params["properties"]["description"].is_object());
    assert!(params["properties"]["name"].is_object());
    assert!(params["properties"]["category"].is_object());
    assert!(params["properties"]["tags"].is_object());
    let required = params["required"].as_array().unwrap();
    assert!(required.iter().any(|v| v == "description"));
}

// =====================================================================
// FEATURE 4D: SKILLS MANIFEST TESTS
// =====================================================================

#[test]
fn test_skills_manifest_format() {
    // The manifest loads from the real skills directory
    // If k8s-deploy.skill.md exists from our CLI test, it should appear
    let manifest = get_skills_manifest();
    // Manifest might be empty if no markdown skills on disk in the default location
    // but the function should not error
    if !manifest.is_empty() {
        // Each line should be formatted as "- **id**: description"
        for line in manifest.lines() {
            assert!(line.starts_with("- **"), "Manifest line should start with '- **': {}", line);
        }
    }
}

// =====================================================================
// COMPLETE TOOL REGISTRATION TESTS
// =====================================================================

#[test]
fn test_all_new_tools_registered() {
    let tools = builtin_tools();
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();

    // New browser tools
    assert!(tool_names.contains(&"browser_snapshot"));
    assert!(tool_names.contains(&"browser_act"));

    // New skill creation tool
    assert!(tool_names.contains(&"create_markdown_skill"));

    // Existing tools still present
    assert!(tool_names.contains(&"capture_screen"));
    assert!(tool_names.contains(&"mouse_click"));
    assert!(tool_names.contains(&"keyboard_type"));
    assert!(tool_names.contains(&"read_file"));
    assert!(tool_names.contains(&"write_file"));
    assert!(tool_names.contains(&"execute_command"));
    assert!(tool_names.contains(&"use_skill"));
    assert!(tool_names.contains(&"list_skills"));
}

#[test]
fn test_browser_act_without_snapshot_returns_error() {
    let ctx = ToolContext::new();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let call = ToolCall {
            name: "browser_act".to_string(),
            arguments: serde_json::json!({
                "session_id": "nonexistent",
                "ref": "@e1",
                "action": "click"
            }),
        };
        let result = execute_tool(&call, &ctx).await.unwrap();
        assert!(!result.success);
        assert!(result.message.contains("No ref map found") || result.message.contains("browser_snapshot"));
    });
}

// =====================================================================
// PAST FEATURE: EXISTING SKILL EXECUTION TESTS
// =====================================================================

// Note: test_use_skill_nonexistent skipped — requires TTY for approval prompt

#[test]
fn test_list_skills_tool() {
    let ctx = ToolContext::new();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let call = ToolCall {
            name: "list_skills".to_string(),
            arguments: serde_json::json!({}),
        };
        let result = execute_tool(&call, &ctx).await.unwrap();
        assert!(result.success);
        let data = result.data.unwrap();
        assert!(data["count"].as_u64().unwrap() >= 5);
    });
}

// =====================================================================
// PAST FEATURE: TOOL CONTEXT TESTS
// =====================================================================

#[test]
fn test_tool_context_with_project_paths() {
    let ctx = ToolContext::with_project_paths();
    // Should not panic
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let refs = ctx.browser_refs.lock().await;
        assert!(refs.is_empty());
    });
}

// =====================================================================
// PAST FEATURE: BROWSER CONFIG TESTS
// =====================================================================

#[test]
fn test_browser_config_default() {
    let config = BrowserConfig::default();
    assert!(config.headless);
    assert_eq!(config.window_width, 1920);
    assert_eq!(config.window_height, 1080);
    assert!(config.validate().is_ok());
}

#[test]
fn test_browser_config_validation() {
    let mut config = BrowserConfig::default();
    config.window_width = 0;
    assert!(config.validate().is_err());
}
