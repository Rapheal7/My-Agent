//! Tool-enabled conversation with automatic tool execution
//!
//! This module provides a conversation handler that automatically
//! executes tools and feeds results back to the LLM.
//! Integrates learning detection, model failover, session compaction,
//! lifecycle hooks, and bootstrap context.

use anyhow::Result;
use std::io::Write;
use std::sync::Arc;
use tracing::{info, warn, debug};

use crate::agent::llm::{OpenRouterClient, ChatMessage, ToolDefinition, FunctionDefinition, ToolCall};
use crate::agent::tools::{ToolContext, ToolCall as AgentToolCall, execute_tool, builtin_tools};
use crate::agent::failover::FailoverClient;
use crate::agent::compaction::SessionCompactor;
use crate::learning::{LearningStore, LearningDetector};
use crate::hooks::{HookRegistry, HookPoint, HookContext, HookAction, should_skip, process_log_actions};

/// Maximum number of tool call rounds before giving up
const MAX_TOOL_ROUNDS: usize = 10;

/// Maximum number of messages before trimming (prevents context overflow)
const MAX_MESSAGES: usize = 50;

/// Number of recent messages to keep during compaction
const COMPACTION_KEEP_RECENT: usize = 15;

/// Roughly estimate token count (approximately 4 chars per token)
fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

/// Estimate total tokens in messages
fn estimate_message_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter()
        .filter_map(|m| m.content.as_ref())
        .map(|c| {
            match c {
                serde_json::Value::String(s) => estimate_tokens(s),
                _ => 0
            }
        })
        .sum()
}

/// Tool-enabled conversation handler with self-improvement integration
pub struct ToolConversation {
    client: OpenRouterClient,
    messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    tool_context: ToolContext,
    model: String,
    /// Enable recursive context compression
    recursive_compression: bool,
    /// Token threshold for recursive compression
    compression_threshold: usize,
    /// Learning store for capturing insights
    learning_store: Option<Arc<LearningStore>>,
    /// Learning detector for automatic detection
    learning_detector: Option<Arc<LearningDetector>>,
    /// Model failover client
    failover: Option<FailoverClient>,
    /// Session compactor for intelligent compression
    compactor: Option<SessionCompactor>,
    /// Lifecycle hook registry
    hooks: Option<Arc<std::sync::Mutex<HookRegistry>>>,
    /// Session ID for hooks
    session_id: String,
    /// Last assistant message (for detection)
    last_assistant_msg: String,
}

impl ToolConversation {
    /// Create a new tool conversation
    pub fn new(client: OpenRouterClient, tool_context: ToolContext) -> Self {
        let tools = Self::convert_tools(builtin_tools());

        Self {
            client,
            messages: Vec::new(),
            tools,
            tool_context,
            model: crate::agent::llm::TEXT_CHAT_MODEL.to_string(),
            recursive_compression: true,
            compression_threshold: 20000,
            learning_store: None,
            learning_detector: None,
            failover: None,
            compactor: None,
            hooks: None,
            session_id: uuid::Uuid::new_v4().to_string(),
            last_assistant_msg: String::new(),
        }
    }

    /// Create with custom model
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Enable or disable recursive compression
    pub fn with_recursive_compression(mut self, enabled: bool) -> Self {
        self.recursive_compression = enabled;
        self
    }

    /// Set compression threshold (in tokens)
    pub fn with_compression_threshold(mut self, threshold: usize) -> Self {
        self.compression_threshold = threshold;
        self
    }

    /// Set system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        let system_msg = ChatMessage::system(prompt);
        self.messages.push(system_msg);
        self
    }

    /// Set bootstrap context (prepended to system prompt)
    pub fn with_bootstrap_context(mut self, bootstrap_context: &str) -> Self {
        if bootstrap_context.is_empty() {
            return self;
        }

        // Find existing system prompt and prepend bootstrap context
        if let Some(sys_msg) = self.messages.iter_mut().find(|m| {
            m.role.as_ref().and_then(|r| r.as_str()) == Some("system")
        }) {
            let existing = sys_msg.content.as_ref()
                .and_then(|c| c.as_str())
                .unwrap_or("");
            let combined = format!("{}\n\n---\n\n{}", bootstrap_context, existing);
            sys_msg.content = Some(serde_json::Value::String(combined));
        } else {
            // No system prompt yet, add bootstrap as system message
            self.messages.insert(0, ChatMessage::system(bootstrap_context));
        }
        self
    }

    /// Set learning store for self-improvement
    pub fn with_learning(mut self, store: Arc<LearningStore>) -> Self {
        let detector = Arc::new(LearningDetector::new(store.clone()));
        self.learning_store = Some(store);
        self.learning_detector = Some(detector);
        self
    }

    /// Set failover client
    pub fn with_failover(mut self, failover: FailoverClient) -> Self {
        self.failover = Some(failover);
        self
    }

    /// Set session compactor
    pub fn with_compactor(mut self, compactor: SessionCompactor) -> Self {
        self.compactor = Some(compactor);
        self
    }

    /// Set hook registry
    pub fn with_hooks(mut self, hooks: Arc<std::sync::Mutex<HookRegistry>>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// Convert agent tools to LLM tool definitions
    fn convert_tools(agent_tools: Vec<crate::agent::tools::Tool>) -> Vec<ToolDefinition> {
        agent_tools.into_iter().map(|t| {
            ToolDefinition {
                r#type: "function".to_string(),
                function: FunctionDefinition {
                    name: t.name,
                    description: t.description,
                    parameters: t.parameters,
                },
            }
        }).collect()
    }

    /// Fire a hook point if hooks are configured
    fn fire_hook(&self, point: HookPoint, data: Vec<(&str, serde_json::Value)>) -> Vec<HookAction> {
        if let Some(ref hooks) = self.hooks {
            let mut ctx = HookContext::new(point, &self.session_id);
            for (key, value) in data {
                ctx = ctx.with_data(key, value);
            }
            if let Ok(registry) = hooks.lock() {
                let actions = registry.fire(point, &ctx);
                process_log_actions(&actions);
                return actions;
            }
        }
        Vec::new()
    }

    /// Process user input with potential tool calls
    pub async fn process(&mut self, user_input: &str) -> Result<String> {
        // Fire BeforePromptBuild hook
        let actions = self.fire_hook(HookPoint::BeforePromptBuild, vec![
            ("user_input", serde_json::json!(user_input)),
        ]);
        if should_skip(&actions) {
            return Ok("Operation skipped by hook.".to_string());
        }

        // Detect learnings from user corrections (comparing to last assistant message)
        if let Some(ref detector) = self.learning_detector {
            if !self.last_assistant_msg.is_empty() {
                let events = detector.detect_from_response(user_input, &self.last_assistant_msg);
                for event in events {
                    let _ = detector.process_event(event);
                }
                // Check for missing capability indicators
                if let Some(event) = detector.detect_missing_capability(user_input, &self.last_assistant_msg) {
                    let _ = detector.process_event(event);
                }
            }
        }

        // Add user message
        self.messages.push(ChatMessage::user(user_input));

        // Check if intelligent compaction is needed (before trimming)
        if let Some(ref compactor) = self.compactor {
            if SessionCompactor::should_compact(&self.messages, MAX_MESSAGES, self.compression_threshold) {
                match compactor.compact(&self.messages, COMPACTION_KEEP_RECENT).await {
                    Ok(compacted) => {
                        info!("Session compacted: {} -> {} messages", self.messages.len(), compacted.len());
                        self.messages = compacted;
                    }
                    Err(e) => {
                        warn!("Compaction failed, falling back to trim: {}", e);
                        self.trim_messages_if_needed();
                    }
                }
            }
        }

        // Main tool-calling loop
        let mut rounds = 0;
        loop {
            if rounds >= MAX_TOOL_ROUNDS {
                warn!("Max tool rounds reached");
                return Ok("I've made several tool calls but haven't reached a conclusion. Please clarify your request.".to_string());
            }
            rounds += 1;

            // Trim messages if too many (fallback if no compactor)
            if self.compactor.is_none() {
                self.trim_messages_if_needed();
            }

            let token_estimate = estimate_message_tokens(&self.messages);
            debug!("Sending request to LLM (round {}, {} messages, ~{} tokens)",
                   rounds, self.messages.len(), token_estimate);

            // Fire BeforeResponse hook
            self.fire_hook(HookPoint::BeforeResponse, vec![
                ("round", serde_json::json!(rounds)),
                ("token_estimate", serde_json::json!(token_estimate)),
            ]);

            // Get response from LLM with tools (using failover if available)
            let response = if let Some(ref failover) = self.failover {
                failover.complete_with_failover(
                    "chat",
                    &self.model,
                    self.messages.clone(),
                    self.tools.clone(),
                    Some(2048),
                ).await?
            } else {
                self.client.complete_with_tools(
                    &self.model,
                    self.messages.clone(),
                    self.tools.clone(),
                    Some(2048),
                ).await?
            };

            // Check if response has tool calls
            if let Some(tool_calls) = &response.tool_calls {
                if !tool_calls.is_empty() {
                    info!("LLM requested {} tool calls", tool_calls.len());

                    // Add assistant message with tool calls to history
                    self.messages.push(response.clone());

                    // Execute each tool call
                    for tool_call in tool_calls {
                        let tool_name = &tool_call.function.name;

                        // Fire BeforeToolExecution hook
                        let actions = self.fire_hook(HookPoint::BeforeToolExecution, vec![
                            ("tool_name", serde_json::json!(tool_name)),
                            ("tool_args", serde_json::json!(tool_call.function.arguments)),
                        ]);
                        if should_skip(&actions) {
                            let skip_msg = ChatMessage::tool_result(
                                &tool_call.id,
                                "Tool execution skipped by hook"
                            );
                            self.messages.push(skip_msg);
                            continue;
                        }

                        let start_time = std::time::Instant::now();

                        match self.execute_tool_call(tool_call).await {
                            Ok(result) => {
                                let duration_ms = start_time.elapsed().as_millis() as u64;

                                // Fire AfterToolExecution hook
                                self.fire_hook(HookPoint::AfterToolExecution, vec![
                                    ("tool_name", serde_json::json!(tool_name)),
                                    ("success", serde_json::json!(true)),
                                    ("duration_ms", serde_json::json!(duration_ms)),
                                ]);

                                // Detect learnings from tool success
                                if let Some(ref detector) = self.learning_detector {
                                    if let Some(event) = detector.detect_from_tool_success(
                                        tool_name, duration_ms, &result
                                    ) {
                                        let _ = detector.process_event(event);
                                    }
                                }

                                // Add tool result to messages
                                let tool_result_msg = ChatMessage::tool_result(
                                    &tool_call.id,
                                    &result
                                );
                                self.messages.push(tool_result_msg);
                            }
                            Err(e) => {
                                let error_str = e.to_string();
                                warn!("Tool execution failed: {}", error_str);

                                // Fire OnError hook
                                self.fire_hook(HookPoint::OnError, vec![
                                    ("tool_name", serde_json::json!(tool_name)),
                                    ("error", serde_json::json!(error_str)),
                                ]);

                                // Detect learnings from tool failure
                                if let Some(ref detector) = self.learning_detector {
                                    if let Some(event) = detector.detect_from_tool_failure(
                                        tool_name, &error_str
                                    ) {
                                        let _ = detector.process_event(event);
                                    }
                                }

                                let error_msg = ChatMessage::tool_result(
                                    &tool_call.id,
                                    format!("Error: {}", e)
                                );
                                self.messages.push(error_msg);
                            }
                        }
                    }

                    // Continue loop to let LLM process results
                    continue;
                }
            }

            // No tool calls - return the response content
            let content = response.content
                .as_ref()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .unwrap_or_default();
            self.messages.push(response);

            // Store last assistant message for correction detection
            self.last_assistant_msg = content.clone();

            // Fire AfterResponse hook
            self.fire_hook(HookPoint::AfterResponse, vec![
                ("response", serde_json::json!(content)),
                ("rounds", serde_json::json!(rounds)),
            ]);

            return Ok(content);
        }
    }

    /// Trim messages to prevent context overflow
    /// Keeps the first message (system prompt) and trims older messages
    fn trim_messages_if_needed(&mut self) {
        if self.messages.len() <= MAX_MESSAGES {
            return;
        }

        // Keep first message (system prompt) and most recent messages
        let keep_count = MAX_MESSAGES - 1;
        let remove_count = self.messages.len() - MAX_MESSAGES;

        // Remove messages from position 1 onwards (keep system prompt)
        if remove_count > 0 && self.messages.len() > 1 {
            self.messages.drain(1..remove_count + 1);
            warn!("Trimmed {} old messages to prevent context overflow", remove_count);
        }
    }

    /// Compress messages using recursive summarization (MIT RLM approach)
    pub async fn compress_recursively(&mut self) -> Result<()> {
        let token_count = estimate_message_tokens(&self.messages);

        if token_count < self.compression_threshold {
            debug!("Token count {} under threshold {}, no compression needed",
                   token_count, self.compression_threshold);
            return Ok(());
        }

        info!("Compressing {} messages (~{} tokens) using recursive summarization",
              self.messages.len(), token_count);

        // Keep system prompt separate
        let system_msg = self.messages.iter()
            .find(|m| m.role.as_ref().and_then(|r| r.as_str()) == Some("system"))
            .cloned();

        // Get messages to compress (exclude system prompt)
        let messages_to_compress: Vec<_> = self.messages.iter()
            .filter(|m| m.role.as_ref().and_then(|r| r.as_str()) != Some("system"))
            .cloned()
            .collect();

        if messages_to_compress.is_empty() {
            return Ok(());
        }

        // Create recursive context manager
        let config = crate::memory::recursive::RecursiveConfig {
            model: self.model.clone(),
            ..Default::default()
        };
        let recursive_mgr = crate::memory::recursive::RecursiveContextManager::with_config(
            self.client.clone(),
            config,
        );

        // Process messages recursively
        let result = recursive_mgr.process_conversation(&messages_to_compress).await?;

        info!("Recursive compression complete: {} -> {} tokens ({:.1}x compression)",
              result.original_tokens, result.final_tokens, result.compression_ratio);

        // Rebuild message list with compressed summary
        self.messages.clear();

        // Add back system prompt
        if let Some(sys) = system_msg {
            self.messages.push(sys);
        }

        // Add compressed context as a system message
        let compressed_msg = ChatMessage::system(format!(
            "Previous conversation summary:\n{}\n\n---\nContinue the conversation based on this context.",
            result.final_summary
        ));
        self.messages.push(compressed_msg);

        Ok(())
    }

    /// Get token count estimate
    pub fn token_count(&self) -> usize {
        estimate_message_tokens(&self.messages)
    }

    /// Check if compression is needed
    pub fn needs_compression(&self) -> bool {
        estimate_message_tokens(&self.messages) > self.compression_threshold
    }

    /// Execute a single tool call
    async fn execute_tool_call(&self, tool_call: &ToolCall) -> Result<String> {
        let name = &tool_call.function.name;
        let args = &tool_call.function.arguments;

        info!("Executing tool: {}", name);

        // Parse arguments
        let arguments: serde_json::Value = serde_json::from_str(args)
            .unwrap_or_else(|_| serde_json::json!({}));

        // Handle learning tools
        if let Some(result) = self.handle_learning_tool(name, &arguments)? {
            return Ok(result);
        }

        // Convert to AgentToolCall
        let agent_call = AgentToolCall {
            name: name.clone(),
            arguments,
        };

        // Execute the tool
        let result = execute_tool(&agent_call, &self.tool_context).await?;

        // Format result as string for the LLM
        let output = if result.success {
            result.message
        } else {
            format!("Error: {}", result.message)
        };

        Ok(output)
    }

    /// Handle learning-specific tools
    fn handle_learning_tool(&self, name: &str, args: &serde_json::Value) -> Result<Option<String>> {
        let store = match &self.learning_store {
            Some(s) => s,
            None => return Ok(None),
        };

        match name {
            "record_learning" => {
                let area = args["area"].as_str().unwrap_or("general");
                let title = args["title"].as_str().unwrap_or("Untitled");
                let description = args["description"].as_str().unwrap_or("");
                let context = args["context"].as_str().unwrap_or("");
                let priority = match args["priority"].as_str().unwrap_or("medium") {
                    "low" => crate::learning::Priority::Low,
                    "high" => crate::learning::Priority::High,
                    "critical" => crate::learning::Priority::Critical,
                    _ => crate::learning::Priority::Medium,
                };

                match store.record_learning(area, title, description, context, None, vec![], priority) {
                    Ok(entry) => Ok(Some(format!("Learning recorded: {} - {}", entry.id, entry.title))),
                    Err(e) => Ok(Some(format!("Failed to record learning: {}", e))),
                }
            }
            "review_learnings" => {
                let status_filter = args["status"].as_str().unwrap_or("all");
                let entries = match status_filter {
                    "new" => store.get_by_status(&crate::learning::EntryStatus::New)?,
                    "validated" => store.get_by_status(&crate::learning::EntryStatus::Validated)?,
                    "promoted" => store.get_by_status(&crate::learning::EntryStatus::Promoted)?,
                    _ => {
                        let mut all = store.get_all(&crate::learning::EntryType::Learning)?;
                        all.extend(store.get_all(&crate::learning::EntryType::Error)?);
                        all.extend(store.get_all(&crate::learning::EntryType::FeatureRequest)?);
                        all
                    }
                };

                if entries.is_empty() {
                    return Ok(Some("No learnings found.".to_string()));
                }

                let mut output = format!("Found {} entries:\n\n", entries.len());
                for entry in entries.iter().take(20) {
                    output.push_str(&format!(
                        "- {} [{}] ({}, {}) — {}\n",
                        entry.id, entry.status, entry.priority, entry.area, entry.title
                    ));
                }
                Ok(Some(output))
            }
            "search_learnings" => {
                let query = args["query"].as_str().unwrap_or("");
                if query.is_empty() {
                    return Ok(Some("Please provide a search query.".to_string()));
                }
                let results = store.search(query)?;
                if results.is_empty() {
                    return Ok(Some(format!("No results for '{}'", query)));
                }
                let mut output = format!("Found {} results for '{}':\n\n", results.len(), query);
                for entry in results.iter().take(10) {
                    output.push_str(&format!(
                        "- {} [{}] — {}: {}\n",
                        entry.id, entry.status, entry.title, entry.description
                    ));
                }
                Ok(Some(output))
            }
            _ => Ok(None), // Not a learning tool
        }
    }

    /// Get conversation history
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// Clear conversation history (except system prompt)
    pub fn clear_history(&mut self) {
        let system_prompt = self.messages.iter()
            .find(|m| m.role.as_ref().and_then(|r| r.as_str()) == Some("system"))
            .cloned();

        self.messages.clear();

        if let Some(sys) = system_prompt {
            self.messages.push(sys);
        }
    }
}

/// Start an interactive tool-enabled chat session
pub async fn start_tool_chat() -> Result<()> {
    println!("Starting tool-enabled chat...");
    println!("The agent can now use tools to help you.");
    println!("Type 'exit' or 'quit' to end, 'clear' to reset history.\n");

    // Check if API key is set
    if !crate::security::keyring::has_api_key() {
        println!("Error: No API key set.");
        println!("Run: my-agent config --set-api-key YOUR_KEY");
        return Ok(());
    }

    // Create client and context
    let client = OpenRouterClient::from_keyring()?;
    let tool_context = ToolContext::new();

    // Initialize learning store
    let learning_store = LearningStore::new().ok().map(Arc::new);

    // Initialize bootstrap context
    let bootstrap_context = crate::learning::BootstrapContext::new()
        .ok()
        .map(|ctx| {
            let _ = ctx.seed_defaults();
            ctx.load_all()
        })
        .unwrap_or_default();

    // Initialize failover client
    let config = crate::config::Config::load().unwrap_or_default();
    let failover = FailoverClient::from_config(client.clone(), &config.failover)
        .with_default_chains();

    // Initialize compactor
    let compactor = SessionCompactor::from_config(client.clone());

    let system_prompt = format!(
        "You are a helpful AI assistant with access to tools. \
You can read files, write files, execute commands, and fetch URLs. \
Always prioritize safety: \
- Confirm destructive operations before executing \
- Be careful with file deletions \
- Validate paths before access \
- Explain what tools you're using and why\n\nCurrent working directory: {}",
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string())
    );

    let mut conversation = ToolConversation::new(client, tool_context)
        .with_system_prompt(system_prompt)
        .with_bootstrap_context(&bootstrap_context)
        .with_failover(failover)
        .with_compactor(compactor);

    if let Some(store) = learning_store {
        conversation = conversation.with_learning(store);
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    loop {
        print!("\n> ");
        stdout.flush()?;

        let mut input = String::new();
        stdin.read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        match input {
            "exit" | "quit" => {
                println!("Goodbye!");
                break;
            }
            "clear" => {
                conversation.clear_history();
                println!("Conversation history cleared.");
                continue;
            }
            _ => {}
        }

        // Process with potential tool calls
        match conversation.process(input).await {
            Ok(response) => {
                println!("\n{}", response);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
            }
        }
    }

    Ok(())
}
