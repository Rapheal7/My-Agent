//! Test for the Smart Reasoning Orchestrator and Agent Spawner

use my_agent::orchestrator::{SmartReasoningOrchestrator, AgentSpawner, ExecutionMode, create_agent_spec};
use my_agent::orchestrator::context::SharedContext;
use my_agent::agent::llm::OpenRouterClient;
use std::sync::Arc;

#[tokio::test]
async fn test_orchestrator_with_kimi() -> anyhow::Result<()> {
    println!("🚀 Testing Agent Spawner with Kimi K2.5 Orchestration\n");

    // Create the orchestrator with Kimi K2.5
    println!("📡 Initializing Smart Reasoning Orchestrator (Kimi K2.5)...");
    let orchestrator = SmartReasoningOrchestrator::new()?;

    // Test request
    let request = "Build a web scraper that extracts product prices from an e-commerce site";
    println!("\n📝 Request: {}", request);

    // Get orchestration plan from Kimi K2.5
    println!("\n🧠 Kimi K2.5 analyzing request...");
    let plan = orchestrator.process_request(request).await?;

    println!("\n📋 Orchestration Plan:");
    println!("   Task Type: {:?}", plan.task_type);
    println!("   Needs Agents: {}", plan.needs_agents);
    println!("   Execution Mode: {:?}", plan.execution_mode);
    println!("   Number of Agents: {}", plan.agents.len());

    if plan.needs_agents && !plan.agents.is_empty() {
        println!("\n🤖 Agent Specifications:");
        for (i, agent) in plan.agents.iter().enumerate() {
            println!("   Agent {}:", i + 1);
            println!("     - Type: {}", agent.capability);
            println!("     - Task: {}", agent.task);
            println!("     - Model: {}", agent.model);
        }

        // Create shared context and spawner
        println!("\n🔧 Creating Agent Spawner...");
        let client = OpenRouterClient::from_keyring()?;
        let context = Arc::new(SharedContext::new(client)?);
        let mut spawner = AgentSpawner::new(context.clone());

        // Spawn agents according to the plan using spawn_batch
        println!("\n🚀 Spawning agents...");
        match spawner.spawn_batch(plan.agents.clone(), ExecutionMode::Parallel).await {
            Ok(ids) => {
                for id in &ids {
                    println!("   ✅ Spawned agent (ID: {})", &id[..8.min(id.len())]);
                }
            }
            Err(e) => {
                println!("   ❌ Failed to spawn agents: {}", e);
            }
        }

        // List active agents
        println!("\n📊 Active Agents:");
        let agents = context.list_agents().await;
        for agent in agents {
            println!("   - {} ({}): {:?}", agent.name, agent.model, agent.status);
        }

        // Shutdown all agents
        println!("\n🛑 Shutting down all agents...");
        spawner.shutdown_all().await?;
        println!("   ✅ All agents shutdown");
    }

    println!("\n✨ Test completed!");
    Ok(())
}

#[tokio::test]
async fn test_simple_agent_spawn() -> anyhow::Result<()> {
    println!("🧪 Testing simple agent spawn...\n");

    let client = OpenRouterClient::from_keyring()?;
    let context = Arc::new(SharedContext::new(client)?);
    let mut spawner = AgentSpawner::new(context.clone());

    // Create a simple agent spec (requires 3 arguments: capability, task, model)
    let spec = create_agent_spec(
        "code",
        "Write a function to calculate fibonacci numbers",
        "qwen/qwen-2.5-coder-32b-instruct"
    );

    println!("🚀 Spawning agent with model: {}", spec.model);
    let ids = spawner.spawn_batch(vec![spec], ExecutionMode::Sequential).await?;

    if let Some(agent_id) = ids.first() {
        println!("✅ Agent spawned: {}", &agent_id[..8.min(agent_id.len())]);

        // Check agent is registered
        let agents = context.list_agents().await;
        assert!(!agents.is_empty(), "At least one agent should be registered");
        println!("✅ Agent registered: {}", agents[0].name);
    }

    // Shutdown
    spawner.shutdown_all().await?;
    println!("✅ Agent shutdown complete");

    Ok(())
}
