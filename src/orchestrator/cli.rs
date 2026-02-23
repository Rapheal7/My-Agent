//! Orchestrator CLI - Interactive multi-agent orchestration
//!
//! Provides a command-line interface for running the Smart Reasoning Orchestrator
//! and managing spawned agent teams.

use crate::orchestrator::{SmartReasoningOrchestrator, ExecutionMode};
use crate::orchestrator::spawner::AgentSpawner;
use crate::orchestrator::context::{SharedContext, AgentStatus};
use crate::orchestrator::bus::AgentMessage;
use crate::agent::llm::OpenRouterClient;
use anyhow::Result;
use std::sync::Arc;
use std::io::{self, Write};
use std::collections::HashMap;
use tracing::{info, debug};

/// Run the orchestrator from CLI
pub async fn run_orchestrator(
    task: Option<String>,
    verbose: bool,
    interactive: bool,
    plan_only: bool,
) -> Result<()> {
    println!("🚀 Smart Reasoning Orchestrator");
    println!("   Powered by Kimi K2.5\n");

    // Get task description
    let task_description = if interactive || task.is_none() {
        println!("Enter your task description:");
        print!("> ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        input.trim().to_string()
    } else {
        task.unwrap()
    };

    if task_description.is_empty() {
        println!("❌ No task provided. Exiting.");
        return Ok(());
    }

    if verbose {
        println!("📋 Task: {}\n", task_description);
    }

    // Initialize orchestrator
    println!("🧠 Initializing orchestrator...");
    let orchestrator = SmartReasoningOrchestrator::new()?;

    // Process request
    println!("🔍 Analyzing task with Kimi K2.5...");
    let plan = orchestrator.process_request(&task_description).await?;

    // Display plan
    println!("\n📊 Orchestration Plan:");
    println!("   Task Type: {:?}", plan.task_type);
    println!("   Execution: {:?}", plan.execution_mode);
    println!("   Agents Needed: {}", if plan.needs_agents { "Yes" } else { "No" });

    // Check if skill creation is needed
    if let Some(ref skill_desc) = plan.skill_needed {
        println!("\n🔧 New Skill Required:");
        println!("   Description: {}", skill_desc);
        if let Some(ref skill_name) = plan.skill_name {
            println!("   Name: {}", skill_name);
        }

        if !plan_only {
            println!("\n⚡ Creating skill...");
            match orchestrator.create_skill(skill_desc, plan.skill_name.as_deref()).await {
                Ok(skill_id) => {
                    println!("✅ Skill created: {}", skill_id);
                    println!("   The skill is now available for use.");
                }
                Err(e) => {
                    println!("❌ Failed to create skill: {}", e);
                    println!("   Continuing with available skills...");
                }
            }
        }
    }

    println!();

    if plan.agents.is_empty() {
        println!("✅ No specialized agents needed for this task.");
        return Ok(());
    }

    println!("🤖 Agent Team:");
    for (i, agent) in plan.agents.iter().enumerate() {
        println!("   {}. {} Agent", i + 1, agent.capability);
        println!("      Task: {}", agent.task);
        println!("      Model: {}\n", agent.model);
    }

    if plan_only {
        println!("📋 Plan-only mode - not executing.");
        return Ok(());
    }

    // Execute plan
    println!("⚡ Executing orchestration plan...\n");

    // Create shared context, bus, and spawner
    let client = OpenRouterClient::from_keyring()?;
    let context = Arc::new(SharedContext::new(client)?);
    let bus = Arc::new(crate::orchestrator::bus::AgentBus::new());
    let mut spawner = AgentSpawner::new(context.clone(), bus.clone());

    // Spawn agents
    let mode = plan.execution_mode;
    let agent_ids = spawner.spawn_batch(plan.agents.clone(), mode).await?;

    println!("✅ Spawned {} agents\n", agent_ids.len());

    // Assign tasks to each agent
    for (i, agent_id) in agent_ids.iter().enumerate() {
        let agent_spec = &plan.agents[i];
        let task_desc = format!("{} - Related to: {}", agent_spec.task, task_description);

        if verbose {
            println!("📤 Assigning task to agent {}: {}", &agent_id[..8], agent_spec.capability);
        }

        let context_json = serde_json::json!({
            "original_request": task_description,
            "agent_type": agent_spec.capability,
            "task_index": i,
        });

        spawner.assign_background(agent_id, task_desc, context_json).await?;
    }

    // Wait for results
    println!("\n⏳ Waiting for agent results...\n");

    // Collect results from task history
    let mut results: HashMap<String, (String, String, bool)> = HashMap::new(); // agent_id -> (name, output, success)
    let mut started = false;

    loop {
        let agents = spawner.list_agents().await;
        let ready_count = agents.iter().filter(|a| matches!(a.status, AgentStatus::Ready)).count();
        let busy_count = agents.iter().filter(|a| matches!(a.status, AgentStatus::Busy)).count();

        // Get task history to find completed tasks
        let task_history = context.get_task_history().await;
        for record in &task_history {
            if let Ok(status_str) = serde_json::to_string(&record.status) {
                let is_completed = status_str.contains("Completed");
                let is_failed = status_str.contains("Failed");
                if (is_completed || is_failed) && !results.contains_key(&record.agent_id) {
                    // Try to get agent info
                    if let Some(agent_info) = context.get_agent(&record.agent_id).await {
                        results.insert(
                            record.agent_id.clone(),
                            (agent_info.name.clone(), record.description.clone(), is_completed)
                        );
                    }
                }
            }
        }

        if verbose {
            debug!("Agents - Ready: {}, Busy: {}", ready_count, busy_count);
        }

        // Check if all tasks are done (agents back to ready)
        if busy_count == 0 && started {
            break;
        }

        if busy_count > 0 {
            started = true;
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    println!("\n✅ All agents completed!");

    // Display results
    if !results.is_empty() {
        println!("\n📊 Agent Results:");
        println!("{}", "─".repeat(60));
        for (i, (agent_id, (name, description, success))) in results.iter().enumerate() {
            let short_id = &agent_id[..8];
            let status = if *success { "✅" } else { "❌" };
            println!("\n{} Agent {} ({})", status, i + 1, name);
            println!("   ID: {}", short_id);
            println!("   Task: {}", description);
        }
        println!("\n{}", "─".repeat(60));
    }

    // Get session stats
    let stats = context.get_stats().await;
    println!("\n📈 Session Statistics:");
    println!("   Total Agents: {}", stats.agent_count);
    println!("   Total Tasks: {}", stats.task_count);
    println!("   Duration: {}s", stats.duration_seconds);

    // Shutdown all agents
    println!("\n🛑 Shutting down agents...");
    spawner.shutdown_all().await?;

    println!("\n🎉 Orchestration complete!");

    Ok(())
}
