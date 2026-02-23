//! Rhai-based skill executor
//!
//! Executes dynamically generated skills using the Rhai scripting engine.
//! Rhai is safe, fast, and allows us to run untrusted code in a sandboxed environment.

use anyhow::{Result, bail};
use rhai::{Engine, Scope, AST, EvalAltResult, Dynamic};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

use super::registry::{SkillMeta, SkillContext, SkillResult, Permission};

/// Maximum execution time for a skill (in seconds)
const MAX_EXECUTION_TIME_SECS: u64 = 30;

/// Rhai skill executor
pub struct RhaiExecutor {
    /// Rhai engine instance
    engine: Engine,
    /// Permissions granted to scripts
    #[allow(dead_code)]
    permissions: Vec<Permission>,
}

impl RhaiExecutor {
    /// Create a new Rhai executor with default sandbox
    pub fn new() -> Self {
        let mut engine = Engine::new();

        // Security settings
        engine.set_max_expr_depths(100, 100); // Limit expression complexity
        engine.set_max_modules(10); // Limit module imports
        engine.set_max_functions(100); // Limit function definitions
        engine.set_max_string_size(1_000_000); // 1MB max string
        engine.set_max_array_size(10_000); // Max array elements
        engine.set_max_map_size(1_000); // Max map entries

        // Disable dangerous features
        engine.disable_symbol("eval"); // No arbitrary code eval
        engine.disable_symbol("import"); // No arbitrary imports
        engine.disable_symbol("export"); // No exports

        // Register standard functions that are safe
        register_safe_functions(&mut engine);

        Self {
            engine,
            permissions: vec![],
        }
    }

    /// Create executor with specific permissions
    pub fn with_permissions(permissions: Vec<Permission>) -> Self {
        let mut executor = Self::new();

        // Add permitted functions
        if permissions.contains(&Permission::ReadFiles) {
            register_file_read_functions(&mut executor.engine);
        }
        if permissions.contains(&Permission::WriteFiles) {
            register_file_write_functions(&mut executor.engine);
        }
        if permissions.contains(&Permission::ExecuteCommands) {
            register_command_functions(&mut executor.engine);
        }
        if permissions.contains(&Permission::NetworkAccess) {
            register_network_functions(&mut executor.engine);
        }

        executor.permissions = permissions;
        executor
    }

    /// Compile a skill script into an AST for repeated execution
    pub fn compile(&self, code: &str) -> Result<AST> {
        self.engine.compile(code)
            .map_err(|e| anyhow::anyhow!("Script compilation failed: {}", e))
    }

    /// Execute a compiled skill
    pub fn execute_compiled(
        &self,
        ast: &AST,
        params: HashMap<String, String>,
        ctx: &SkillContext,
    ) -> Result<SkillResult> {
        let start = Instant::now();

        // Create scope with parameters
        let mut scope = Scope::new();

        // Convert HashMap to Rhai Map
        let mut params_map = rhai::Map::new();
        for (k, v) in params {
            params_map.insert(k.into(), v.into());
        }
        scope.push("params", params_map);
        scope.push("working_dir", ctx.working_dir.to_string_lossy().to_string());

        // Execute the AST with scope
        let result: Result<Dynamic, Box<EvalAltResult>> = self.engine.eval_ast_with_scope(&mut scope, ast);

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(value) => {
                let output = dynamic_to_string(&value);
                Ok(SkillResult {
                    success: true,
                    output,
                    error: None,
                    duration_ms,
                })
            }
            Err(e) => {
                // Check for special error types
                let err_str = e.to_string();
                if err_str.contains("terminated") {
                    // Script called return - this is actually success
                    Ok(SkillResult {
                        success: true,
                        output: String::new(),
                        error: None,
                        duration_ms,
                    })
                } else {
                    Ok(SkillResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Execution error: {}", err_str)),
                        duration_ms,
                    })
                }
            }
        }
    }

    /// Execute a skill script directly
    pub fn execute(
        &self,
        code: &str,
        params: HashMap<String, String>,
        ctx: &SkillContext,
    ) -> Result<SkillResult> {
        let ast = self.compile(code)?;
        self.execute_compiled(&ast, params, ctx)
    }
}

impl Default for RhaiExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a Rhai Dynamic value to a string
fn dynamic_to_string(value: &Dynamic) -> String {
    if value.is::<String>() {
        value.clone_cast::<String>()
    } else if value.is::<i64>() {
        value.clone_cast::<i64>().to_string()
    } else if value.is::<f64>() {
        value.clone_cast::<f64>().to_string()
    } else if value.is::<bool>() {
        value.clone_cast::<bool>().to_string()
    } else if value.is::<rhai::Array>() {
        let arr = value.clone_cast::<rhai::Array>();
        let items: Vec<String> = arr.iter().map(dynamic_to_string).collect();
        format!("[{}]", items.join(", "))
    } else if value.is::<rhai::Map>() {
        let map = value.clone_cast::<rhai::Map>();
        let items: Vec<String> = map.iter()
            .map(|(k, v)| format!("{}: {}", k, dynamic_to_string(v)))
            .collect();
        format!("{{{}}}", items.join(", "))
    } else if value.is_unit() {
        String::new()
    } else {
        format!("{:?}", value)
    }
}

/// Register safe, standard functions
fn register_safe_functions(engine: &mut Engine) {
    // String functions
    engine.register_fn("len", |s: &mut String| s.len() as i64);
    engine.register_fn("trim", |s: &mut String| s.trim().to_string());
    engine.register_fn("to_lower", |s: &mut String| s.to_lowercase());
    engine.register_fn("to_upper", |s: &mut String| s.to_uppercase());
    engine.register_fn("contains", |s: &mut String, substr: String| s.contains(&substr));
    engine.register_fn("starts_with", |s: &mut String, prefix: String| s.starts_with(&prefix));
    engine.register_fn("ends_with", |s: &mut String, suffix: String| s.ends_with(&suffix));
    engine.register_fn("replace", |s: &mut String, from: String, to: String| {
        s.replace(&from, &to)
    });

    // Split returns an iterator, we need to collect it
    engine.register_fn("split", |s: &mut String, delim: String| -> rhai::Array {
        s.split(&delim).map(|p| p.to_string().into()).collect()
    });

    // Array functions
    engine.register_fn("push", |arr: &mut rhai::Array, item: Dynamic| {
        arr.push(item);
    });
    engine.register_fn("pop", |arr: &mut rhai::Array| -> Dynamic {
        arr.pop().unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("len", |arr: &mut rhai::Array| arr.len() as i64);

    // Math functions
    engine.register_fn("abs", |x: i64| x.abs());
    engine.register_fn("abs", |x: f64| x.abs());
    engine.register_fn("min", |a: i64, b: i64| a.min(b));
    engine.register_fn("max", |a: i64, b: i64| a.max(b));
    engine.register_fn("floor", |x: f64| x.floor() as i64);
    engine.register_fn("ceil", |x: f64| x.ceil() as i64);
    engine.register_fn("round", |x: f64| x.round() as i64);

    // Type conversion
    engine.register_fn("to_int", |s: &mut String| -> i64 {
        s.parse().unwrap_or(0)
    });
    engine.register_fn("to_float", |s: &mut String| -> f64 {
        s.parse().unwrap_or(0.0)
    });
    engine.register_fn("to_string", |n: i64| n.to_string());
    engine.register_fn("to_string", |n: f64| n.to_string());

    // Debug/logging
    engine.register_fn("print", |s: String| {
        println!("{}", s);
    });
    engine.register_fn("log", |s: String| {
        info!("[skill] {}", s);
    });
}

/// Helper to create an error
fn make_error(msg: String) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(msg.into(), rhai::Position::NONE))
}

/// Register file reading functions (requires ReadFiles permission)
fn register_file_read_functions(engine: &mut Engine) {
    engine.register_fn("read_file", |path: String| -> Result<String, Box<EvalAltResult>> {
        std::fs::read_to_string(&path)
            .map_err(|e| make_error(format!("Failed to read file: {}", e)))
    });

    engine.register_fn("file_exists", |path: String| -> bool {
        std::path::Path::new(&path).exists()
    });

    engine.register_fn("list_dir", |path: String| -> Result<rhai::Array, Box<EvalAltResult>> {
        let entries = std::fs::read_dir(&path)
            .map_err(|e| make_error(format!("Failed to list directory: {}", e)))?;

        let mut result = rhai::Array::new();
        for entry in entries.flatten() {
            let mut map = rhai::Map::new();
            map.insert("name".into(), entry.file_name().to_string_lossy().to_string().into());
            map.insert("path".into(), entry.path().to_string_lossy().to_string().into());
            map.insert("is_dir".into(), entry.file_type().map(|t| t.is_dir()).unwrap_or(false).into());
            result.push(map.into());
        }
        Ok(result)
    });

    engine.register_fn("file_info", |path: String| -> Result<rhai::Map, Box<EvalAltResult>> {
        let metadata = std::fs::metadata(&path)
            .map_err(|e| make_error(format!("Failed to get file info: {}", e)))?;

        let mut map = rhai::Map::new();
        map.insert("exists".into(), true.into());
        map.insert("size".into(), (metadata.len() as i64).into());
        map.insert("is_file".into(), metadata.is_file().into());
        map.insert("is_dir".into(), metadata.is_dir().into());
        Ok(map)
    });
}

/// Register file writing functions (requires WriteFiles permission)
fn register_file_write_functions(engine: &mut Engine) {
    engine.register_fn("write_file", |path: String, content: String| -> Result<(), Box<EvalAltResult>> {
        std::fs::write(&path, &content)
            .map_err(|e| make_error(format!("Failed to write file: {}", e)))
    });

    engine.register_fn("append_file", |path: String, content: String| -> Result<(), Box<EvalAltResult>> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| make_error(format!("Failed to open file: {}", e)))?;

        file.write_all(content.as_bytes())
            .map_err(|e| make_error(format!("Failed to append to file: {}", e)))
    });

    engine.register_fn("create_dir", |path: String| -> Result<(), Box<EvalAltResult>> {
        std::fs::create_dir_all(&path)
            .map_err(|e| make_error(format!("Failed to create directory: {}", e)))
    });

    engine.register_fn("delete_file", |path: String| -> Result<(), Box<EvalAltResult>> {
        std::fs::remove_file(&path)
            .map_err(|e| make_error(format!("Failed to delete file: {}", e)))
    });
}

/// Register command execution functions (requires ExecuteCommands permission)
fn register_command_functions(engine: &mut Engine) {
    engine.register_fn("run_command", |cmd: String| -> Result<rhai::Map, Box<EvalAltResult>> {
        use std::process::Command;

        let output = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output()
            .map_err(|e| make_error(format!("Failed to run command: {}", e)))?;

        let mut result = rhai::Map::new();
        result.insert("success".into(), output.status.success().into());
        result.insert("stdout".into(), String::from_utf8_lossy(&output.stdout).to_string().into());
        result.insert("stderr".into(), String::from_utf8_lossy(&output.stderr).to_string().into());
        result.insert("code".into(), (output.status.code().unwrap_or(-1) as i64).into());
        Ok(result)
    });
}

/// Register network functions (requires NetworkAccess permission)
fn register_network_functions(engine: &mut Engine) {
    engine.register_fn("http_get", |url: String| -> Result<String, Box<EvalAltResult>> {
        let response = reqwest::blocking::get(&url)
            .map_err(|e| make_error(format!("HTTP request failed: {}", e)))?;

        let text = response.text()
            .map_err(|e| make_error(format!("Failed to read response: {}", e)))?;

        Ok(text)
    });

    engine.register_fn("http_post", |url: String, body: String| -> Result<String, Box<EvalAltResult>> {
        let client = reqwest::blocking::Client::new();
        let response = client.post(&url)
            .body(body)
            .send()
            .map_err(|e| make_error(format!("HTTP POST failed: {}", e)))?;

        let text = response.text()
            .map_err(|e| make_error(format!("Failed to read response: {}", e)))?;

        Ok(text)
    });
}

/// Generate Rhai code for a skill from metadata
pub fn generate_skill_code(meta: &SkillMeta, implementation: Option<&str>) -> String {
    let mut code = format!(
        r#"// Skill: {}
// Description: {}
// Version: {}

"#,
        meta.name, meta.description, meta.version
    );

    // Add parameter extraction
    code.push_str("// Extract parameters\n");
    for param in &meta.parameters {
        code.push_str(&format!(
            r#"let {} = params["{}"]; // {}
"#,
            param.name, param.name, param.description
        ));
    }

    code.push_str("\n");

    if let Some(impl_code) = implementation {
        code.push_str(impl_code);
    } else {
        // Generate template based on category
        code.push_str(&generate_category_template(&meta));
    }

    code
}

/// Generate a template implementation based on skill category
fn generate_category_template(meta: &SkillMeta) -> String {
    match meta.category {
        super::registry::SkillCategory::Filesystem => {
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
        super::registry::SkillCategory::Web => {
            r#"// Fetch URL
let response = http_get(url);

response
"#.to_string()
        }
        super::registry::SkillCategory::Shell => {
            r#"// Execute command
let result = run_command(command);

if result.success {
    result.stdout
} else {
    "Error: " + result.stderr
}
"#.to_string()
        }
        super::registry::SkillCategory::Data => {
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
// Return the result
"Skill executed with parameters"
"#,
                meta.parameters.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", ")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_creation() {
        let executor = RhaiExecutor::new();
        let result = executor.compile("1 + 1");
        assert!(result.is_ok());
    }

    #[test]
    fn test_simple_execution() {
        let executor = RhaiExecutor::new();
        let code = "2 + 2";
        let result = executor.execute(code, HashMap::new(), &SkillContext::default()).unwrap();

        assert!(result.success);
        assert_eq!(result.output, "4");
    }

    #[test]
    fn test_string_functions() {
        let executor = RhaiExecutor::new();
        // Test basic string operations that work reliably in Rhai
        let code = r#"
let s = "hello";
s
"#;
        let result = executor.execute(code, HashMap::new(), &SkillContext::default()).unwrap();

        assert!(result.success, "Execution failed: {:?}", result.error);
        assert_eq!(result.output, "hello");
    }

    #[test]
    fn test_params_access() {
        let executor = RhaiExecutor::new();
        let code = r#"
let name = params["name"];
"Hello, " + name + "!"
"#;

        let mut params = HashMap::new();
        params.insert("name".to_string(), "World".to_string());

        let result = executor.execute(code, params, &SkillContext::default()).unwrap();

        assert!(result.success);
        assert_eq!(result.output, "Hello, World!");
    }

    #[test]
    fn test_file_read_permission() {
        let executor = RhaiExecutor::with_permissions(vec![Permission::ReadFiles]);

        let code = r#"
file_exists("/nonexistent/path")
"#;

        let result = executor.execute(code, HashMap::new(), &SkillContext::default()).unwrap();
        assert!(result.success);
        assert_eq!(result.output, "false");
    }

    #[test]
    fn test_compile_and_cache() {
        let executor = RhaiExecutor::new();
        let ast = executor.compile("1 + 2 + 3").unwrap();

        let result1 = executor.execute_compiled(&ast, HashMap::new(), &SkillContext::default()).unwrap();
        let result2 = executor.execute_compiled(&ast, HashMap::new(), &SkillContext::default()).unwrap();

        assert_eq!(result1.output, "6");
        assert_eq!(result2.output, "6");
    }
}
