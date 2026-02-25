//! Shared tool-calling loop (ReAct pattern)
//!
//! Extracted from interactive.rs so both interactive mode AND subagents
//! use the same proven logic.

use anyhow::Result;
use std::collections::HashSet;
use crate::agent::llm::{ChatMessage, OpenRouterClient, ToolDefinition, FunctionDefinition};
use crate::agent::tools::{Tool, ToolContext, ToolCall, execute_tool};

/// Configuration for the tool-calling loop
pub struct ToolLoopConfig {
    /// LLM model to use
    pub model: String,
    /// System prompt
    pub system_prompt: String,
    /// Tools available to this loop
    pub allowed_tools: Vec<Tool>,
    /// Maximum ReAct iterations before stopping
    pub max_iterations: usize,
    /// Max tokens per LLM call
    pub max_tokens: u32,
    /// Optional callback when a tool starts executing
    pub on_tool_start: Option<Box<dyn Fn(&str) + Send + Sync>>,
    /// Optional callback when a tool completes (name, success, message)
    pub on_tool_complete: Option<Box<dyn Fn(&str, bool, &str) + Send + Sync>>,
    /// Optional callback for progress updates
    pub on_progress: Option<Box<dyn Fn(&str) + Send + Sync>>,
}

/// Result from a completed tool loop
pub struct ToolLoopResult {
    /// The final text response from the LLM
    pub final_response: String,
    /// Number of ReAct iterations executed
    pub iterations: usize,
    /// Total number of tool calls made across all iterations
    pub tool_calls_made: usize,
    /// Whether the loop completed successfully (vs hitting max iterations)
    pub success: bool,
}

/// Run the ReAct tool-calling loop.
///
/// 1. Send messages + tool definitions to LLM
/// 2. If tool_calls present, execute each, add results to messages, continue
/// 3. If no tool_calls, return content as final_response
/// 4. Max iterations guard
pub async fn run_tool_loop(
    client: &OpenRouterClient,
    initial_messages: Vec<ChatMessage>,
    tool_ctx: &ToolContext,
    config: &ToolLoopConfig,
) -> Result<ToolLoopResult> {
    let tool_defs: Vec<ToolDefinition> = config.allowed_tools.iter().map(|t| ToolDefinition {
        r#type: "function".to_string(),
        function: FunctionDefinition {
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.parameters.clone(),
        },
    }).collect();

    // Build messages with system prompt
    let mut messages = vec![ChatMessage::system(&config.system_prompt)];
    messages.extend(initial_messages);

    let mut iteration = 0;
    let mut total_tool_calls = 0;
    let mut final_response = String::new();
    // Track tool calls to detect repeated identical calls
    let mut seen_calls: HashSet<String> = HashSet::new();
    let mut consecutive_dupes = 0;
    const MAX_CONSECUTIVE_DUPES: usize = 2;

    loop {
        iteration += 1;
        if iteration > config.max_iterations {
            if let Some(ref cb) = config.on_progress {
                cb("Maximum tool iterations reached, stopping.");
            }
            return Ok(ToolLoopResult {
                final_response,
                iterations: iteration - 1,
                tool_calls_made: total_tool_calls,
                success: false,
            });
        }

        if let Some(ref cb) = config.on_progress {
            cb(&format!("Iteration {}/{}", iteration, config.max_iterations));
        }

        // Call LLM with tools
        let response = client.complete_with_tools(
            &config.model,
            messages.clone(),
            tool_defs.clone(),
            Some(config.max_tokens),
        ).await?;

        // Check for tool calls
        let tool_calls = response.tool_calls.clone();
        let has_tool_calls = tool_calls.as_ref().map(|tc| !tc.is_empty()).unwrap_or(false);

        if !has_tool_calls {
            // No tool calls - extract final response
            final_response = response.content
                .as_ref()
                .and_then(|c| c.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            break;
        }

        // Execute tool calls
        let tool_calls = tool_calls.unwrap();
        total_tool_calls += tool_calls.len();

        // Add assistant message with tool calls
        let assistant_msg = ChatMessage {
            role: Some(serde_json::json!("assistant")),
            content: response.content.clone(),
            reasoning_details: None,
            tool_calls: Some(tool_calls.clone()),
            tool_call_id: None,
            name: None,
            reasoning: None,
            refusal: None,
        };
        messages.push(assistant_msg);

        // Check for repeated identical tool calls (deduplication)
        let call_keys: Vec<String> = tool_calls.iter()
            .map(|tc| format!("{}:{}", tc.function.name, tc.function.arguments))
            .collect();
        let all_dupes = call_keys.iter().all(|k| seen_calls.contains(k));
        if all_dupes {
            consecutive_dupes += 1;
            if consecutive_dupes >= MAX_CONSECUTIVE_DUPES {
                if let Some(ref cb) = config.on_progress {
                    cb("Stopping: LLM is repeating the same tool calls.");
                }
                // Return whatever we have so far
                return Ok(ToolLoopResult {
                    final_response,
                    iterations: iteration,
                    tool_calls_made: total_tool_calls,
                    success: false,
                });
            }
        } else {
            consecutive_dupes = 0;
        }
        for key in &call_keys {
            seen_calls.insert(key.clone());
        }

        // Execute each tool call and collect results
        for tc in &tool_calls {
            let call = ToolCall {
                name: tc.function.name.clone(),
                arguments: serde_json::from_str(&tc.function.arguments).unwrap_or_default(),
            };

            if let Some(ref cb) = config.on_tool_start {
                cb(&call.name);
            }

            let tool_result_content = match execute_tool(&call, tool_ctx).await {
                Ok(result) => {
                    if let Some(ref cb) = config.on_tool_complete {
                        cb(&call.name, result.success, &result.message);
                    }
                    if let Some(data) = &result.data {
                        serde_json::to_string(data).unwrap_or_else(|_| result.message.clone())
                    } else {
                        result.message.clone()
                    }
                }
                Err(e) => {
                    if let Some(ref cb) = config.on_tool_complete {
                        cb(&call.name, false, &format!("Error: {}", e));
                    }
                    format!("Error: {}", e)
                }
            };

            let tool_result_msg = ChatMessage {
                role: Some(serde_json::json!("tool")),
                content: Some(serde_json::json!(tool_result_content)),
                reasoning_details: None,
                tool_calls: None,
                tool_call_id: Some(tc.id.clone()),
                name: Some(call.name.clone()),
                reasoning: None,
                refusal: None,
            };
            messages.push(tool_result_msg);
        }
    }

    Ok(ToolLoopResult {
        final_response,
        iterations: iteration,
        tool_calls_made: total_tool_calls,
        success: true,
    })
}
