//! Manual test for create_skill tool
//!
//! Run with: cargo run --example test_create_skill

use my_agent::agent::tools::{Tool, ToolCall, ToolContext, builtin_tools, execute_tool};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Testing create_skill tool ===\n");

    // Show available tools
    println!("1. Available tools:");
    let tools = builtin_tools();
    for tool in &tools {
        println!("   - {}:", tool.name);
        println!("     {}", tool.description.lines().next().unwrap_or(""));
    }
    println!();

    // Verify create_skill is in the list
    let has_create_skill = tools.iter().any(|t| t.name == "create_skill");
    println!("2. create_skill tool present: {}", has_create_skill);
    println!();

    // Create tool context
    let ctx = ToolContext::new();

    // Test 1: List existing skills
    println!("3. Testing list_skills:");
    let list_call = ToolCall {
        name: "list_skills".to_string(),
        arguments: serde_json::json!({}),
    };

    match execute_tool(&list_call, &ctx).await {
        Ok(result) => {
            println!("   Success: {}", result.success);
            println!("   Message: {}", result.message);
            if let Some(data) = result.data {
                if let Some(skills) = data.get("skills").and_then(|s| s.as_array()) {
                    println!("   Skills found:");
                    for skill in skills {
                        println!("     - {} ({})",
                            skill["name"].as_str().unwrap_or("unknown"),
                            skill["id"].as_str().unwrap_or("unknown")
                        );
                    }
                }
            }
        }
        Err(e) => println!("   Error: {}", e),
    }
    println!();

    // Test 2: Create a new skill
    println!("4. Testing create_skill:");
    println!("   Description: 'Convert JSON to YAML format'");
    let create_call = ToolCall {
        name: "create_skill".to_string(),
        arguments: serde_json::json!({
            "description": "Convert JSON data to YAML format",
            "name": "json-to-yaml",
            "category": "Data"
        }),
    };

    match execute_tool(&create_call, &ctx).await {
        Ok(result) => {
            println!("   Success: {}", result.success);
            println!("   Message: {}", result.message);
            if let Some(data) = result.data {
                println!("   Skill ID: {}", data["skill_id"].as_str().unwrap_or("unknown"));
                println!("   Name: {}", data["name"].as_str().unwrap_or("unknown"));
                println!("   Category: {}", data["category"].as_str().unwrap_or("unknown"));
            }
        }
        Err(e) => println!("   Error: {}", e),
    }
    println!();

    // Test 3: List skills again to see the new one
    println!("5. Listing skills after creation:");
    match execute_tool(&list_call, &ctx).await {
        Ok(result) => {
            println!("   {}", result.message);
            if let Some(data) = result.data {
                if let Some(skills) = data.get("skills").and_then(|s| s.as_array()) {
                    println!("   Skills:");
                    for skill in skills {
                        let builtin = skill["builtin"].as_bool().unwrap_or(false);
                        let marker = if builtin { "[built-in]" } else { "[dynamic]" };
                        println!("     {} {} ({})",
                            marker,
                            skill["name"].as_str().unwrap_or("unknown"),
                            skill["id"].as_str().unwrap_or("unknown")
                        );
                    }
                }
            }
        }
        Err(e) => println!("   Error: {}", e),
    }
    println!();

    // Test 4: Try to use the created skill
    println!("6. Testing use_skill with created skill:");
    let use_call = ToolCall {
        name: "use_skill".to_string(),
        arguments: serde_json::json!({
            "skill_id": "json-to-yaml",
            "params": {
                "input": "{\"hello\": \"world\"}"
            }
        }),
    };

    match execute_tool(&use_call, &ctx).await {
        Ok(result) => {
            println!("   Success: {}", result.success);
            println!("   Message: {}", result.message);
            if let Some(data) = result.data {
                println!("   Output: {}", data["output"].as_str().unwrap_or("none"));
            }
        }
        Err(e) => println!("   Error: {}", e),
    }
    println!();

    println!("=== Test complete ===");
    Ok(())
}
