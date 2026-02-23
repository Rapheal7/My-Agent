//! Test self-improvement tools
use my_agent::agent::tools::{ToolCall, ToolContext, execute_tool};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Testing Self-Improvement Tools ===\n");

    let ctx = ToolContext::new();

    // Test 1: analyze_performance
    println!("1. Testing analyze_performance:");
    let analyze_call = ToolCall {
        name: "analyze_performance".to_string(),
        arguments: serde_json::json!({"focus": "all"}),
    };

    match execute_tool(&analyze_call, &ctx).await {
        Ok(result) => {
            println!("   Success: {}", result.success);
            println!("   Message: {}", result.message.lines().next().unwrap_or(""));
        }
        Err(e) => println!("   Error: {}", e),
    }
    println!();

    // Test 2: get_lessons
    println!("2. Testing get_lessons:");
    let lessons_call = ToolCall {
        name: "get_lessons".to_string(),
        arguments: serde_json::json!({"context": ""}),
    };

    match execute_tool(&lessons_call, &ctx).await {
        Ok(result) => {
            println!("   Success: {}", result.success);
            println!("   Message: {}", result.message);
        }
        Err(e) => println!("   Error: {}", e),
    }
    println!();

    // Test 3: record_lesson
    println!("3. Testing record_lesson:");
    let record_call = ToolCall {
        name: "record_lesson".to_string(),
        arguments: serde_json::json!({
            "insight": "Files with .rs extension are Rust source files",
            "context": "When exploring Rust projects",
            "related_tools": ["read_file", "glob"]
        }),
    };

    match execute_tool(&record_call, &ctx).await {
        Ok(result) => {
            println!("   Success: {}", result.success);
            println!("   Message: {}", result.message);
        }
        Err(e) => println!("   Error: {}", e),
    }
    println!();

    // Test 4: improve_self
    println!("4. Testing improve_self:");
    let improve_call = ToolCall {
        name: "improve_self".to_string(),
        arguments: serde_json::json!({"area": "all"}),
    };

    match execute_tool(&improve_call, &ctx).await {
        Ok(result) => {
            println!("   Success: {}", result.success);
            println!("   Message: {}", result.message.lines().take(5).collect::<Vec<_>>().join("\n"));
        }
        Err(e) => println!("   Error: {}", e),
    }
    println!();

    println!("=== Test complete ===");
    Ok(())
}
