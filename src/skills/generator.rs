//! LLM-based skill code generation
//!
//! Generates skill implementations dynamically using LLM.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

use super::registry::{
    SkillMeta, SkillCategory, Permission, SkillParameter, ParameterType, Skill, SkillResult,
};

/// Skill generation request
#[derive(Debug, Clone, Serialize)]
pub struct GenerationRequest {
    /// Description of what the skill should do
    pub description: String,
    /// Desired skill name
    pub name: Option<String>,
    /// Category hint
    pub category: Option<SkillCategory>,
    /// Required permissions hint
    pub permissions: Vec<Permission>,
    /// Example inputs/outputs
    pub examples: Vec<Example>,
}

/// Example for skill generation
#[derive(Debug, Clone, Serialize)]
pub struct Example {
    /// Input parameters
    pub input: HashMap<String, String>,
    /// Expected output
    pub output: String,
}

/// Generated skill definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedSkill {
    /// Skill metadata
    pub meta: SkillMeta,
    /// Generated code (Python-like pseudocode)
    pub code: String,
    /// Explanation of how the skill works
    pub explanation: String,
}

/// Skill generator using LLM
pub struct SkillGenerator {
    /// OpenRouter API key
    api_key: Option<String>,
    /// Model to use for generation
    model: String,
}

impl SkillGenerator {
    /// Create a new skill generator
    pub fn new() -> Self {
        Self {
            api_key: None,
            model: "openrouter/pony-alpha".to_string(),
        }
    }

    /// Set API key
    pub fn with_api_key(mut self, key: String) -> Self {
        self.api_key = Some(key);
        self
    }

    /// Set model
    pub fn with_model(mut self, model: String) -> Self {
        self.model = model;
        self
    }

    /// Generate a skill from a description
    pub async fn generate(&self, request: GenerationRequest) -> Result<GeneratedSkill> {
        let api_key = self.api_key.clone().or_else(|| {
            std::env::var("OPENROUTER_API_KEY").ok()
        });

        let Some(api_key) = api_key else {
            // Return a template-based skill if no API key
            return self.generate_template(request);
        };

        let prompt = self.build_prompt(&request);

        // Call OpenRouter API
        let client = reqwest::Client::new();
        let response = client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("HTTP-Referer", "https://github.com/my-agent")
            .json(&serde_json::json!({
                "model": self.model,
                "messages": [
                    {"role": "system", "content": SKILL_SYSTEM_PROMPT},
                    {"role": "user", "content": prompt}
                ],
                "temperature": 0.7,
                "max_tokens": 2000
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            warn!("LLM generation failed, falling back to template");
            return self.generate_template(request);
        }

        let body = response.text().await?;
        let generated = self.parse_llm_response(&body, &request)?;

        info!("Generated skill: {}", generated.meta.name);
        Ok(generated)
    }

    /// Build the generation prompt
    fn build_prompt(&self, request: &GenerationRequest) -> String {
        let mut prompt = format!(
            "Generate a skill definition for the following:\n\n**Description:** {}\n",
            request.description
        );

        if let Some(ref name) = request.name {
            prompt.push_str(&format!("**Suggested name:** {}\n", name));
        }

        if let Some(ref category) = request.category {
            prompt.push_str(&format!("**Category:** {:?}\n", category));
        }

        if !request.permissions.is_empty() {
            prompt.push_str("**Required permissions:** ");
            let perms: Vec<String> = request.permissions.iter().map(|p| format!("{:?}", p)).collect();
            prompt.push_str(&perms.join(", "));
            prompt.push('\n');
        }

        if !request.examples.is_empty() {
            prompt.push_str("\n**Examples:**\n");
            for (i, example) in request.examples.iter().enumerate() {
                prompt.push_str(&format!("\n{}. Input: {:?}\n   Output: {}\n", i + 1, example.input, example.output));
            }
        }

        prompt.push_str("\nGenerate a JSON skill definition with the following structure:\n");
        prompt.push_str("- meta: skill metadata (id, name, description, version, category, permissions, parameters, tags)\n");
        prompt.push_str("- code: Python-like pseudocode implementation\n");
        prompt.push_str("- explanation: how the skill works\n");

        prompt
    }

    /// Parse LLM response into a GeneratedSkill
    fn parse_llm_response(&self, body: &str, _request: &GenerationRequest) -> Result<GeneratedSkill> {
        let value: serde_json::Value = serde_json::from_str(body)?;

        let content = value["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No content in LLM response"))?;

        // Try to extract JSON from the response
        let json_str = if content.contains("```json") {
            let start = content.find("```json").unwrap() + 7;
            let end = content[start..].find("```").unwrap() + start;
            &content[start..end]
        } else if content.contains("```") {
            let start = content.find("```").unwrap() + 3;
            let end = content[start..].find("```").unwrap() + start;
            &content[start..end]
        } else {
            content
        };

        let generated: GeneratedSkill = serde_json::from_str(json_str.trim())
            .map_err(|e| anyhow::anyhow!("Failed to parse generated skill: {}", e))?;

        Ok(generated)
    }

    /// Generate a template-based skill (fallback without LLM)
    fn generate_template(&self, request: GenerationRequest) -> Result<GeneratedSkill> {
        let name = request.name.unwrap_or_else(|| {
            // Generate a name from description
            let words: Vec<&str> = request.description.split_whitespace().take(3).collect();
            words.join("-").to_lowercase().replace(' ', "-")
        });

        let id = name.to_lowercase().replace(' ', "-");

        let category = request.category.unwrap_or(SkillCategory::Utility);

        let permissions = if request.permissions.is_empty() {
            vec![Permission::ReadFiles]
        } else {
            request.permissions
        };

        // Generate basic parameters based on description
        let parameters = self.infer_parameters(&request.description);

        let meta = SkillMeta {
            id: id.clone(),
            name: name.clone(),
            description: request.description.clone(),
            version: "1.0.0".to_string(),
            author: Some("my-agent".to_string()),
            category,
            permissions,
            parameters,
            builtin: false,
            tags: self.infer_tags(&request.description),
        };

        // Generate template code
        let code = self.generate_template_code(&meta, &request.examples);

        let explanation = format!(
            "This skill {}.\n\nGenerated as a template. Customize the implementation as needed.",
            request.description
        );

        Ok(GeneratedSkill {
            meta,
            code,
            explanation,
        })
    }

    /// Infer parameters from description
    fn infer_parameters(&self, description: &str) -> Vec<SkillParameter> {
        let mut params = Vec::new();
        let lower = description.to_lowercase();

        // Look for common patterns
        if lower.contains("file") || lower.contains("path") {
            params.push(SkillParameter {
                name: "path".to_string(),
                param_type: ParameterType::Path,
                required: true,
                default: None,
                description: "File or directory path".to_string(),
                allowed_values: None,
            });
        }

        if lower.contains("url") || lower.contains("http") || lower.contains("api") {
            params.push(SkillParameter {
                name: "url".to_string(),
                param_type: ParameterType::Url,
                required: true,
                default: None,
                description: "URL to access".to_string(),
                allowed_values: None,
            });
        }

        if lower.contains("search") || lower.contains("query") || lower.contains("find") {
            params.push(SkillParameter {
                name: "query".to_string(),
                param_type: ParameterType::String,
                required: true,
                default: None,
                description: "Search query".to_string(),
                allowed_values: None,
            });
        }

        // Always add a generic input parameter
        if params.is_empty() {
            params.push(SkillParameter {
                name: "input".to_string(),
                param_type: ParameterType::String,
                required: false,
                default: None,
                description: "Input data".to_string(),
                allowed_values: None,
            });
        }

        params
    }

    /// Infer tags from description
    fn infer_tags(&self, description: &str) -> Vec<String> {
        let mut tags = Vec::new();
        let lower = description.to_lowercase();

        let tag_keywords = [
            ("file", "file"),
            ("read", "read"),
            ("write", "write"),
            ("web", "web"),
            ("api", "api"),
            ("search", "search"),
            ("data", "data"),
            ("convert", "convert"),
            ("parse", "parse"),
            ("download", "download"),
            ("upload", "upload"),
        ];

        for (keyword, tag) in tag_keywords {
            if lower.contains(keyword) {
                tags.push(tag.to_string());
            }
        }

        tags
    }

    /// Generate template code in Rhai
    fn generate_template_code(&self, meta: &SkillMeta, examples: &[Example]) -> String {
        use super::executor::generate_skill_code;

        // Generate base template
        let mut code = format!(
            "// Skill: {}\n// Description: {}\n\n",
            meta.name, meta.description
        );

        // Add parameter extraction
        code.push_str("// Extract parameters\n");
        for param in &meta.parameters {
            code.push_str(&format!(
                "let {} = params[\"{}\"];\n",
                param.name, param.name
            ));
        }
        code.push_str("\n");

        // Add examples as comments
        if !examples.is_empty() {
            code.push_str("// Examples:\n");
            for example in examples {
                code.push_str(&format!("// Input: {:?}\n", example.input));
                code.push_str(&format!("// Output: {}\n", example.output));
            }
            code.push_str("\n");
        }

        // Add implementation based on category and permissions
        code.push_str(&self.generate_category_implementation(meta));

        code
    }

    /// Generate implementation based on skill category
    fn generate_category_implementation(&self, meta: &SkillMeta) -> String {
        match meta.category {
            SkillCategory::Filesystem => {
                if meta.permissions.contains(&Permission::WriteFiles) {
                    r#"// Write to file
write_file(path, content);

"File written successfully to: " + path
"#.to_string()
                } else {
                    r#"// Read file
let content = read_file(path);

content
"#.to_string()
                }
            }
            SkillCategory::Web => {
                r#"// Fetch URL
let response = http_get(url);

response
"#.to_string()
            }
            SkillCategory::Shell => {
                r#"// Execute command
let result = run_command(command);

if result.success {
    result.stdout
} else {
    "Error: " + result.stderr
}
"#.to_string()
            }
            SkillCategory::Data => {
                r#"// Process data
let items = input.split(",");

let result = [];
for item in items {
    result.push(item.trim());
}

result
"#.to_string()
            }
            _ => {
                format!(
                    r#"// TODO: Implement skill logic
// Parameters: {}

"Skill executed successfully"
"#,
                    meta.parameters.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", ")
                )
            }
        }
    }

    /// Compile a generated skill into an executable Skill
    pub fn compile_skill(&self, generated: &GeneratedSkill) -> Result<Skill> {
        use super::executor::RhaiExecutor;

        // Create executor with the skill's permissions
        let executor = RhaiExecutor::with_permissions(generated.meta.permissions.clone());

        // Compile the script
        let full_code = super::executor::generate_skill_code(&generated.meta, Some(&generated.code));
        let ast = executor.compile(&full_code)?;

        // Store the AST and executor for execution
        let executor = std::sync::Arc::new(executor);
        let ast = std::sync::Arc::new(ast);

        let skill = Skill::new(generated.meta.clone(), move |params, ctx| {
            let executor = executor.clone();
            let ast = ast.clone();

            executor.execute_compiled(&ast, params, ctx)
        });

        Ok(skill)
    }
}

impl Default for SkillGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// System prompt for skill generation
const SKILL_SYSTEM_PROMPT: &str = r#"You are a skill code generator for an AI agent system.
Generate skill definitions in JSON format with the following structure:

{
  "meta": {
    "id": "skill-id",
    "name": "Skill Name",
    "description": "What this skill does",
    "version": "1.0.0",
    "author": "author-name",
    "category": "Utility",
    "permissions": ["ReadFiles"],
    "parameters": [
      {
        "name": "param1",
        "param_type": "String",
        "required": true,
        "default": null,
        "description": "Parameter description",
        "allowed_values": null
      }
    ],
    "builtin": false,
    "tags": ["tag1", "tag2"]
  },
  "code": "// Rhai script code\nlet param1 = params[\"param1\"];\n// Implementation\nresult",
  "explanation": "How the skill works"
}

The code field must contain Rhai script code (similar to JavaScript/Rust).
Available functions based on permissions:
- ReadFiles: read_file(path), file_exists(path), list_dir(path), file_info(path)
- WriteFiles: write_file(path, content), append_file(path, content), create_dir(path), delete_file(path)
- ExecuteCommands: run_command(cmd) returns {success, stdout, stderr, code}
- NetworkAccess: http_get(url), http_post(url, body)

Built-in functions always available:
- String: len(), trim(), to_lower(), to_upper(), contains(substr), split(delim), replace(from, to)
- Math: abs(), min(), max(), floor(), ceil(), round()
- Type conversion: to_int(), to_float(), to_string()
- Debug: print(msg), log(msg)

Parameters are accessed via: params["param_name"]

Categories: Filesystem, Shell, Web, Data, System, Utility, Custom
Permissions: ReadFiles, WriteFiles, ExecuteCommands, NetworkAccess, ReadEnvironment, SystemModify
ParameterTypes: String, Integer, Float, Boolean, Path, Url, Enum, Array, Object

Generate practical, working skills with actual implementation code.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generator_creation() {
        let generator = SkillGenerator::new();
        assert!(generator.api_key.is_none());
    }

    #[test]
    fn test_infer_parameters() {
        let generator = SkillGenerator::new();

        let params = generator.infer_parameters("Read a file from disk");
        assert!(params.iter().any(|p| p.name == "path"));

        let params = generator.infer_parameters("Fetch data from a URL");
        assert!(params.iter().any(|p| p.name == "url"));

        let params = generator.infer_parameters("Search for files");
        assert!(params.iter().any(|p| p.name == "query"));
    }

    #[test]
    fn test_infer_tags() {
        let generator = SkillGenerator::new();

        let tags = generator.infer_tags("Read and parse a file from disk");
        assert!(tags.contains(&"file".to_string()));
        assert!(tags.contains(&"read".to_string()));
        assert!(tags.contains(&"parse".to_string()));
    }

    #[tokio::test]
    async fn test_template_generation() {
        let generator = SkillGenerator::new();

        let request = GenerationRequest {
            description: "Read a file and return its contents".to_string(),
            name: Some("file-reader".to_string()),
            category: Some(SkillCategory::Filesystem),
            permissions: vec![Permission::ReadFiles],
            examples: vec![],
        };

        let result = generator.generate(request).await.unwrap();

        assert_eq!(result.meta.id, "file-reader");
        assert_eq!(result.meta.category, SkillCategory::Filesystem);
        assert!(result.meta.permissions.contains(&Permission::ReadFiles));
    }

    #[test]
    fn test_compile_skill() {
        let generator = SkillGenerator::new();

        let generated = GeneratedSkill {
            meta: SkillMeta {
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
            },
            // Valid Rhai code (not Python)
            code: r#"let result = "test"; result"#.to_string(),
            explanation: "A test skill".to_string(),
        };

        let skill = generator.compile_skill(&generated).unwrap();
        assert_eq!(skill.meta.id, "test");
    }
}
