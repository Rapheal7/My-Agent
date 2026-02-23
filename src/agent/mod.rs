//! Agent module - conversation and LLM interaction

pub mod conversation;
pub mod llm;
pub mod tools;
pub mod tool_conversation;
pub mod tool_loop;
pub mod interactive;
pub mod context_manager;
pub mod failover;
pub mod compaction;

use anyhow::Result;
use std::io::{self, Write};
use std::sync::Arc;

/// Start a text chat session
pub async fn start_text_chat() -> Result<()> {
    start_text_chat_with_options(false, None, false).await
}

/// Start a text chat session with optional persistence
pub async fn start_text_chat_with_options(
    persistent: bool,
    conversation_id: Option<String>,
    resume: bool,
) -> Result<()> {
    println!("Starting text chat with OpenRouter...");
    if persistent {
        println!("Persistence enabled - conversations will be saved.");
    }
    println!("Type 'exit' or 'quit' to end the session.");
    println!("Type 'history' to see conversation history.");
    println!("Type 'clear' to start a new conversation.\n");

    // Check if API key is set
    if !crate::security::keyring::has_api_key() {
        println!("Error: No API key set.");
        println!("Run: my-agent config --set-api-key YOUR_KEY");
        return Ok(());
    }

    // Create client
    let client = llm::OpenRouterClient::from_keyring()?;

    // Initialize memory store if persistence is enabled
    let memory_store = if persistent {
        match crate::memory::MemoryStore::default_store().await {
            Ok(store) => {
                println!("Memory store initialized.\n");
                Some(Arc::new(store))
            }
            Err(e) => {
                println!("Warning: Could not initialize memory store: {}", e);
                println!("Continuing without persistence.\n");
                None
            }
        }
    } else {
        None
    };

    // Create conversation
    let mut conversation = if let (Some(id), Some(store)) = (conversation_id.as_ref(), memory_store.as_ref()) {
        // Load existing conversation
        match store.load_conversation(id).await? {
            Some(record) => {
                println!("Loaded conversation: {} ({} messages)", id, record.messages.len());
                conversation::Conversation::from_record(record)
            }
            None => {
                println!("Conversation {} not found. Starting new conversation.", id);
                conversation::Conversation::new()
            }
        }
    } else if resume {
        // Try to resume most recent conversation
        if let Some(ref store) = memory_store {
            let recent = store.list_conversations(1, 0).await?;
            if let Some(record) = recent.first() {
                println!("Resuming conversation: {} ({} messages)", record.id, record.messages.len());
                conversation::Conversation::from_record(record.clone())
            } else {
                conversation::Conversation::new()
            }
        } else {
            conversation::Conversation::new()
        }
    } else {
        conversation::Conversation::new()
    };

    // Add system prompt if this is a new conversation
    if conversation.messages.is_empty() {
        conversation.add_message(
            conversation::Role::System,
            "You are a helpful AI assistant. Be concise and friendly.".to_string()
        );
    }

    println!("Chat ready. Conversation ID: {}", conversation.id);
    println!("Enter your message:");

    loop {
        // Read user input
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            continue;
        }

        // Handle special commands
        match input.to_lowercase().as_str() {
            "exit" | "quit" => {
                // Save conversation before exiting
                if let Some(ref store) = memory_store {
                    if let Err(e) = store.save_conversation(&conversation.to_record()).await {
                        println!("Warning: Could not save conversation: {}", e);
                    }
                }
                println!("Goodbye!");
                break;
            }
            "history" => {
                println!("\n=== Conversation History ===");
                for msg in &conversation.messages {
                    println!("[{}] {}", msg.role, msg.content);
                }
                println!("============================\n");
                continue;
            }
            "clear" | "new" => {
                // Save current conversation
                if let Some(ref store) = memory_store {
                    if let Err(e) = store.save_conversation(&conversation.to_record()).await {
                        println!("Warning: Could not save conversation: {}", e);
                    }
                }
                // Start new conversation
                conversation = conversation::Conversation::new();
                conversation.add_message(
                    conversation::Role::System,
                    "You are a helpful AI assistant. Be concise and friendly.".to_string()
                );
                println!("Started new conversation. ID: {}\n", conversation.id);
                continue;
            }
            "save" => {
                if let Some(ref store) = memory_store {
                    if let Err(e) = store.save_conversation(&conversation.to_record()).await {
                        println!("Error saving conversation: {}", e);
                    } else {
                        println!("Conversation saved. ID: {}\n", conversation.id);
                    }
                } else {
                    println!("Persistence not enabled. Start with --persistent flag.\n");
                }
                continue;
            }
            _ => {}
        }

        // Add user message to conversation
        conversation.add_message(conversation::Role::User, input.to_string());

        // Convert to LLM messages
        let messages: Vec<llm::ChatMessage> = conversation.messages
            .iter()
            .map(|m| llm::ChatMessage {
                role: Some(serde_json::json!(match m.role {
                    conversation::Role::User => "user",
                    conversation::Role::Assistant => "assistant",
                    conversation::Role::System => "system",
                })),
                content: Some(serde_json::json!(m.content.clone())),
                reasoning_details: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning: None,
                refusal: None,
            })
            .collect();

        // Send to LLM
        println!();
        match client.complete(llm::TEXT_CHAT_MODEL, messages, Some(2048)).await {
            Ok(response) => {
                println!("{}\n", response);
                conversation.add_message(conversation::Role::Assistant, response);

                // Auto-save after each exchange if persistence is enabled
                if let Some(ref store) = memory_store {
                    if let Err(e) = store.save_conversation(&conversation.to_record()).await {
                        tracing::warn!("Could not auto-save conversation: {}", e);
                    }
                }
            }
            Err(e) => {
                println!("Error: {}\n", e);
            }
        }
    }

    Ok(())
}

/// List recent conversations
pub async fn list_conversations(limit: usize) -> Result<()> {
    let store = crate::memory::MemoryStore::default_store().await?;
    let conversations = store.list_conversations(limit, 0).await?;

    if conversations.is_empty() {
        println!("No conversations found.");
        return Ok(());
    }

    println!("Recent conversations:\n");
    for (i, conv) in conversations.iter().enumerate() {
        let title = conv.title.as_deref().unwrap_or("Untitled");
        let msg_count = conv.messages.len();
        let date = conv.updated_at.format("%Y-%m-%d %H:%M");
        println!("{}. {} (ID: {}, {} messages, {})", i + 1, title, conv.id, msg_count, date);
    }

    Ok(())
}

/// Search conversations
pub async fn search_conversations(query: &str, limit: usize) -> Result<()> {
    let store = crate::memory::MemoryStore::default_store().await?;
    let results = store.search_conversations(query, limit).await?;

    if results.is_empty() {
        println!("No matching conversations found.");
        return Ok(());
    }

    println!("Found {} matching conversations:\n", results.len());
    for (i, conv) in results.iter().enumerate() {
        let title = conv.title.as_deref().unwrap_or("Untitled");
        let date = conv.updated_at.format("%Y-%m-%d %H:%M");
        println!("{}. {} (ID: {}, {})", i + 1, title, conv.id, date);
    }

    Ok(())
}
