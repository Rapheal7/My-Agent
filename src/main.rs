//! My Agent - Personal AI Assistant
//!
//! A secure personal AI agent with cloud orchestration and skill system.

// Use the library crate for all modules
use my_agent::cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Install Rustls crypto provider for HTTPS support
    // This is required for Rustls 0.23+
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install Rustls crypto provider");

    // Initialize logging (WARN level by default, use RUST_LOG=info for debug)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into())
        )
        .init();

    // Run CLI
    cli::run().await
}
