//! Shared tool-calling loop (ReAct pattern)
//!
//! Extracted from interactive.rs so both interactive mode AND subagents
//! use the same proven logic.

use anyhow::Result;
use std::collections::{HashSet, VecDeque};
use std::time::Duration;
use crate::agent::llm::{ChatMessage, OpenRouterClient, ToolDefinition, FunctionDefinition};
use crate::agent::tools::{Tool, ToolContext, ToolCall, ToolResult, execute_tool};

/// Why the tool loop stopped
#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    /// LLM returned a text response (normal completion)
    Completed,
    /// Hit the maximum iteration count
    MaxIterations,
    /// Wall-clock timeout exceeded
    Timeout,
    /// Loop pattern detected (description of the pattern)
    LoopDetected(String),
    /// Same tool call repeated too many times
    DuplicateCalls,
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopReason::Completed => write!(f, "completed"),
            StopReason::MaxIterations => write!(f, "max iterations reached"),
            StopReason::Timeout => write!(f, "timeout"),
            StopReason::LoopDetected(desc) => write!(f, "loop detected: {}", desc),
            StopReason::DuplicateCalls => write!(f, "duplicate calls"),
        }
    }
}

/// Detects loop patterns in tool call sequences
pub struct LoopDetector {
    /// Sliding window of recent call signatures
    window: VecDeque<String>,
    /// Hashes of results paired with call signatures
    result_hashes: VecDeque<(String, u64)>,
    /// Maximum window size
    max_window: usize,
}

impl LoopDetector {
    pub fn new() -> Self {
        Self {
            window: VecDeque::with_capacity(10),
            result_hashes: VecDeque::with_capacity(10),
            max_window: 10,
        }
    }

    /// Record a call and its result hash, then check for loop patterns.
    /// Returns Some(description) if a loop is detected.
    pub fn check(&mut self, call_sig: &str, result_hash: u64) -> Option<String> {
        // Skip utility/pacing calls that shouldn't affect loop detection
        if call_sig.starts_with("wait:") {
            return None;
        }
        self.window.push_back(call_sig.to_string());
        self.result_hashes.push_back((call_sig.to_string(), result_hash));
        if self.window.len() > self.max_window {
            self.window.pop_front();
        }
        if self.result_hashes.len() > self.max_window {
            self.result_hashes.pop_front();
        }

        if let Some(desc) = self.detect_generic_repeat() {
            return Some(desc);
        }
        if let Some(desc) = self.detect_ping_pong() {
            return Some(desc);
        }
        if let Some(desc) = self.detect_poll_no_progress() {
            return Some(desc);
        }
        None
    }

    /// Same tool+args 3+ times in last 6 calls
    fn detect_generic_repeat(&self) -> Option<String> {
        if self.window.len() < 6 {
            return None;
        }
        let recent: Vec<_> = self.window.iter().rev().take(6).collect();
        let mut counts = std::collections::HashMap::new();
        for sig in &recent {
            *counts.entry(sig.as_str()).or_insert(0u32) += 1;
        }
        for (sig, count) in counts {
            // Observation tools (browser_snapshot, capture_screen) are expected to be
            // called repeatedly as part of the observe→act→verify cycle. Only flag them
            // if they dominate the window (5+ of 6), which indicates a real stuck loop.
            let is_observation = sig.starts_with("browser_snapshot:") || sig.starts_with("capture_screen:");
            let threshold = if is_observation { 5 } else { 3 };
            if count >= threshold {
                return Some(format!("generic_repeat: '{}' called {} times in last 6", sig, count));
            }
        }
        None
    }

    /// Alternating A,B,A,B pattern (4 calls)
    fn detect_ping_pong(&self) -> Option<String> {
        if self.window.len() < 4 {
            return None;
        }
        let w: Vec<_> = self.window.iter().rev().take(4).collect();
        // w is [newest, ..., oldest], reverse to get chronological
        if w[0] == w[2] && w[1] == w[3] && w[0] != w[1] {
            return Some(format!("ping_pong: alternating '{}' and '{}'", w[1], w[0]));
        }
        None
    }

    /// Same call returning same result hash 3+ times
    fn detect_poll_no_progress(&self) -> Option<String> {
        if self.result_hashes.len() < 3 {
            return None;
        }
        let recent: Vec<_> = self.result_hashes.iter().rev().take(6).collect();
        let mut counts = std::collections::HashMap::new();
        for (sig, hash) in &recent {
            *counts.entry((sig.as_str(), *hash)).or_insert(0u32) += 1;
        }
        for ((sig, _), count) in counts {
            if count >= 3 {
                return Some(format!("poll_no_progress: '{}' returned identical results {} times", sig, count));
            }
        }
        None
    }
}

/// Compute a simple hash of a string for result comparison
pub fn hash_result(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Analyze a screenshot using the configured vision model.
/// If the tool result contains screenshot data, sends the image to the vision model
/// and returns a text description that the main (non-vision) model can understand.
/// Returns None if the result is not an image.
async fn analyze_screenshot_with_vision(result: &ToolResult) -> Option<String> {
    let data = result.data.as_ref()?;
    let base64_data = data.get("base64_data")?.as_str()?;
    let media_type = data.get("media_type")?.as_str()?;
    if !media_type.starts_with("image/") {
        return None;
    }
    let width = data.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
    let height = data.get("height").and_then(|v| v.as_u64()).unwrap_or(0);

    // Load config to get vision model
    let config = match crate::config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Vision: failed to load config: {}", e);
            return Some(format!("Screenshot captured: {}x{} (vision unavailable: config error)", width, height));
        }
    };
    let vision_model = config.models.vision.clone();

    // Create OpenRouter client
    let client = match OpenRouterClient::from_keyring() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Vision: failed to create client: {}", e);
            return Some(format!("Screenshot captured: {}x{} (vision unavailable: no API key)", width, height));
        }
    };

    eprintln!("Vision: analyzing screenshot {}x{} with model {}", width, height, vision_model);

    // Build multimodal message with the screenshot
    let messages = vec![
        ChatMessage {
            role: Some(serde_json::json!("user")),
            content: Some(serde_json::json!([
                {
                    "type": "text",
                    "text": "Briefly describe what's on screen: windows/apps visible, their content, any readable text, and interactive UI elements (buttons, links, input fields). Be concise."
                },
                {
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:{};base64,{}", media_type, base64_data)
                    }
                }
            ])),
            reasoning_details: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning: None,
            refusal: None,
        },
    ];

    match client.complete(&vision_model, messages, Some(512)).await {
        Ok(description) if !description.is_empty() => {
            eprintln!("Vision: analysis complete ({} chars)", description.len());
            Some(format!(
                "Screenshot captured: {}x{}\n\nVision analysis (model: {}):\n{}",
                width, height, vision_model, description
            ))
        }
        Ok(_) => {
            eprintln!("Vision: model returned empty response");
            Some(format!("Screenshot captured: {}x{} (vision model {} returned empty response)", width, height, vision_model))
        }
        Err(e) => {
            eprintln!("Vision: analysis failed: {}", e);
            Some(format!("Screenshot captured: {}x{} (vision analysis failed: {})", width, height, e))
        }
    }
}

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
    /// Wall-clock timeout in seconds (0 = no timeout)
    pub timeout_secs: u64,
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
    /// Why the loop stopped
    pub stop_reason: StopReason,
}

/// Run the ReAct tool-calling loop.
///
/// 1. Send messages + tool definitions to LLM
/// 2. If tool_calls present, execute each, add results to messages, continue
/// 3. If no tool_calls, return content as final_response
/// 4. Max iterations guard + wall-clock timeout + loop detection
pub async fn run_tool_loop(
    client: &OpenRouterClient,
    initial_messages: Vec<ChatMessage>,
    tool_ctx: &ToolContext,
    config: &ToolLoopConfig,
) -> Result<ToolLoopResult> {
    let timeout_secs = if config.timeout_secs > 0 {
        config.timeout_secs
    } else {
        900 // default 15 minutes
    };

    match tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        run_tool_loop_inner(client, initial_messages, tool_ctx, config),
    ).await {
        Ok(result) => result,
        Err(_) => {
            if let Some(ref cb) = config.on_progress {
                cb(&format!("Tool loop timed out after {}s", timeout_secs));
            }
            Ok(ToolLoopResult {
                final_response: String::new(),
                iterations: 0,
                tool_calls_made: 0,
                success: false,
                stop_reason: StopReason::Timeout,
            })
        }
    }
}

/// Inner loop implementation (wrapped by timeout)
async fn run_tool_loop_inner(
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
    let mut loop_detector = LoopDetector::new();

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
                stop_reason: StopReason::MaxIterations,
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
            return Ok(ToolLoopResult {
                final_response,
                iterations: iteration,
                tool_calls_made: total_tool_calls,
                success: true,
                stop_reason: StopReason::Completed,
            });
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
                return Ok(ToolLoopResult {
                    final_response,
                    iterations: iteration,
                    tool_calls_made: total_tool_calls,
                    success: false,
                    stop_reason: StopReason::DuplicateCalls,
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

            // For screenshots, route the image through the vision model to get
            // a text description that the main (non-vision) model can understand.
            // Truncate large results to prevent context explosion in subagents.
            // 8000 chars ≈ 2000 tokens — keeps subagent context manageable.
            const MAX_TOOL_RESULT_CHARS: usize = 8000;

            let tool_result_text = match execute_tool(&call, tool_ctx).await {
                Ok(result) => {
                    if let Some(ref cb) = config.on_tool_complete {
                        cb(&call.name, result.success, &result.message);
                    }
                    if let Some(vision_description) = analyze_screenshot_with_vision(&result).await {
                        vision_description
                    } else {
                        let text_content = if let Some(data) = &result.data {
                            // Strip base64_data from serialization to avoid dumping
                            // megabytes of raw image data into the LLM context
                            let clean_data = if data.get("base64_data").is_some() {
                                let mut obj = data.clone();
                                if let Some(map) = obj.as_object_mut() {
                                    map.remove("base64_data");
                                }
                                obj
                            } else {
                                data.clone()
                            };
                            let full = serde_json::to_string(&clean_data).unwrap_or_else(|_| result.message.clone());
                            if full.len() > MAX_TOOL_RESULT_CHARS {
                                format!("{}...\n[truncated: {} total chars]", &full[..MAX_TOOL_RESULT_CHARS], full.len())
                            } else {
                                full
                            }
                        } else {
                            result.message.clone()
                        };
                        text_content
                    }
                }
                Err(e) => {
                    if let Some(ref cb) = config.on_tool_complete {
                        cb(&call.name, false, &format!("Error: {}", e));
                    }
                    format!("Error: {}", e)
                }
            };

            // Check for loop patterns via LoopDetector
            let call_sig = format!("{}:{}", call.name, tc.function.arguments);
            let result_h = hash_result(&tool_result_text);
            if let Some(loop_desc) = loop_detector.check(&call_sig, result_h) {
                if let Some(ref cb) = config.on_progress {
                    cb(&format!("Loop detected: {}", loop_desc));
                }
                let tool_result_msg = ChatMessage {
                    role: Some(serde_json::json!("tool")),
                    content: Some(serde_json::json!(tool_result_text)),
                    reasoning_details: None,
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                    name: Some(call.name.clone()),
                    reasoning: None,
                    refusal: None,
                };
                messages.push(tool_result_msg);

                return Ok(ToolLoopResult {
                    final_response,
                    iterations: iteration,
                    tool_calls_made: total_tool_calls,
                    success: false,
                    stop_reason: StopReason::LoopDetected(loop_desc),
                });
            }

            let tool_result_msg = ChatMessage {
                role: Some(serde_json::json!("tool")),
                content: Some(serde_json::json!(tool_result_text)),
                reasoning_details: None,
                tool_calls: None,
                tool_call_id: Some(tc.id.clone()),
                name: Some(call.name.clone()),
                reasoning: None,
                refusal: None,
            };
            messages.push(tool_result_msg);

            // Vision analysis (if any) is already included in tool_result_text,
            // so no separate image message is needed.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stop_reason_display() {
        assert_eq!(format!("{}", StopReason::Completed), "completed");
        assert_eq!(format!("{}", StopReason::MaxIterations), "max iterations reached");
        assert_eq!(format!("{}", StopReason::Timeout), "timeout");
        assert_eq!(format!("{}", StopReason::DuplicateCalls), "duplicate calls");
        assert_eq!(
            format!("{}", StopReason::LoopDetected("ping_pong".into())),
            "loop detected: ping_pong"
        );
    }

    #[test]
    fn test_hash_result_deterministic() {
        let h1 = hash_result("hello world");
        let h2 = hash_result("hello world");
        let h3 = hash_result("different");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_loop_detector_no_detection_with_few_calls() {
        let mut ld = LoopDetector::new();
        // Too few calls to detect any pattern
        assert!(ld.check("read_file:foo.rs", 100).is_none());
        assert!(ld.check("write_file:bar.rs", 200).is_none());
        assert!(ld.check("read_file:baz.rs", 300).is_none());
    }

    #[test]
    fn test_loop_detector_generic_repeat() {
        let mut ld = LoopDetector::new();
        // Need at least 6 calls, with the same signature 3+ times
        ld.check("read_file:a", 1);
        ld.check("read_file:a", 1);
        ld.check("write_file:b", 2);
        ld.check("read_file:a", 1);
        ld.check("write_file:b", 2);
        // At 6 calls, read_file:a appeared 3 times in the last 6
        let result = ld.check("read_file:a", 1);
        assert!(result.is_some(), "Should detect generic_repeat");
        let desc = result.unwrap();
        assert!(desc.contains("generic_repeat"), "Got: {}", desc);
    }

    #[test]
    fn test_loop_detector_ping_pong() {
        let mut ld = LoopDetector::new();
        // A, B, A, B pattern
        ld.check("read_file:x", 10);
        ld.check("write_file:y", 20);
        ld.check("read_file:x", 10);
        let result = ld.check("write_file:y", 20);
        assert!(result.is_some(), "Should detect ping_pong");
        let desc = result.unwrap();
        assert!(desc.contains("ping_pong"), "Got: {}", desc);
    }

    #[test]
    fn test_loop_detector_poll_no_progress() {
        let mut ld = LoopDetector::new();
        let same_hash = hash_result("same result content");
        // Same call + same result hash 3 times
        ld.check("check_status:{}", same_hash);
        ld.check("check_status:{}", same_hash);
        let result = ld.check("check_status:{}", same_hash);
        assert!(result.is_some(), "Should detect poll_no_progress");
        let desc = result.unwrap();
        assert!(desc.contains("poll_no_progress"), "Got: {}", desc);
    }

    #[test]
    fn test_loop_detector_no_false_positive_varied_calls() {
        let mut ld = LoopDetector::new();
        // All different calls — should never trigger
        for i in 0..10 {
            let sig = format!("tool_{}:arg_{}", i, i);
            assert!(ld.check(&sig, i as u64).is_none(),
                "Should not detect loop for varied calls at iteration {}", i);
        }
    }

    #[test]
    fn test_loop_detector_no_false_positive_same_call_different_results() {
        let mut ld = LoopDetector::new();
        // Same call but different results each time — no poll_no_progress
        // But may trigger generic_repeat after 6 calls
        ld.check("read_file:log.txt", 100);
        ld.check("other:x", 200);
        ld.check("read_file:log.txt", 101);
        ld.check("other:y", 201);
        ld.check("read_file:log.txt", 102);
        // 5 calls, read_file appears 3 times but we need 6 total
        let result = ld.check("other:z", 202);
        // read_file:log.txt is only 3 of last 6, may or may not trigger depending on exact window
        // The important point: poll_no_progress should NOT trigger because hashes differ
        if let Some(desc) = result {
            assert!(!desc.contains("poll_no_progress"),
                "Should not detect poll_no_progress with varying results, got: {}", desc);
        }
    }

    #[test]
    fn test_loop_detector_sliding_window_eviction() {
        let mut ld = LoopDetector::new();
        // Fill window with 10 unique calls
        for i in 0..10 {
            ld.check(&format!("tool_{}:arg", i), i as u64);
        }
        assert_eq!(ld.window.len(), 10);

        // Adding one more should evict the oldest
        ld.check("tool_10:arg", 10);
        assert_eq!(ld.window.len(), 10);
        assert_eq!(ld.window.front().unwrap(), "tool_1:arg"); // tool_0 evicted
    }

    #[test]
    fn test_stop_reason_equality() {
        assert_eq!(StopReason::Completed, StopReason::Completed);
        assert_ne!(StopReason::Completed, StopReason::Timeout);
        assert_eq!(
            StopReason::LoopDetected("x".into()),
            StopReason::LoopDetected("x".into())
        );
        assert_ne!(
            StopReason::LoopDetected("x".into()),
            StopReason::LoopDetected("y".into())
        );
    }
}
