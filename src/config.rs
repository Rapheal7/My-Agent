//! Configuration management
//!
//! Manages agent configuration including API settings, security, and budget limits.

use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// OpenRouter API settings
    #[serde(default)]
    pub openrouter: OpenRouterConfig,
    /// Model assignments for different roles
    #[serde(default)]
    pub models: ModelsConfig,
    /// Budget limits
    #[serde(default)]
    pub budget: BudgetConfig,
    /// Security settings
    #[serde(default)]
    pub security: SecurityConfig,
    /// JWT authentication settings
    #[serde(default)]
    pub auth: AuthConfig,
    /// Model failover chain configuration
    #[serde(default)]
    pub failover: crate::agent::failover::FailoverConfig,
    /// Gateway daemon configuration
    #[serde(default)]
    pub gateway: crate::gateway::GatewayConfig,
}

/// Model assignments for different agent roles
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    /// Model for the orchestrator (task planning)
    #[serde(default = "default_orchestrator_model")]
    pub orchestrator: String,
    /// Model for coding tasks
    #[serde(default = "default_code_model")]
    pub code: String,
    /// Model for research tasks
    #[serde(default = "default_research_model")]
    pub research: String,
    /// Model for reasoning tasks
    #[serde(default = "default_reasoning_model")]
    pub reasoning: String,
    /// Model for utility tasks (analysis, file ops, exploration, general, skill creation)
    #[serde(default = "default_utility_model")]
    pub utility: String,
    /// Default chat model
    #[serde(default = "default_chat_model")]
    pub chat: String,
    /// Model for vision tasks (screenshots, images)
    #[serde(default = "default_vision_model")]
    pub vision: String,
}

fn default_orchestrator_model() -> String {
    "z-ai/glm-5".to_string()
}

fn default_code_model() -> String {
    "minimax/minimax-m2.5".to_string()
}

fn default_research_model() -> String {
    "openai/gpt-oss-120b:free".to_string()
}

fn default_reasoning_model() -> String {
    "openai/gpt-oss-120b:free".to_string()
}

fn default_utility_model() -> String {
    "z-ai/glm-5".to_string()
}

fn default_chat_model() -> String {
    "z-ai/glm-5".to_string()
}

fn default_vision_model() -> String {
    "google/gemini-flash-1.5".to_string()
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            orchestrator: default_orchestrator_model(),
            code: default_code_model(),
            research: default_research_model(),
            reasoning: default_reasoning_model(),
            utility: default_utility_model(),
            chat: default_chat_model(),
            vision: default_vision_model(),
        }
    }
}

impl ModelsConfig {
    /// Get model for a role name
    pub fn get(&self, role: &str) -> Option<&str> {
        match role.to_lowercase().as_str() {
            "orchestrator" => Some(&self.orchestrator),
            "code" | "coder" => Some(&self.code),
            "research" | "researcher" => Some(&self.research),
            "reasoning" => Some(&self.reasoning),
            // Consolidated into utility: analysis, file, general, explorer, skill-creator
            "utility" | "analysis" | "analyst" | "file" | "filesystem" | "general" | "explore" | "explorer" | "skill-creator" | "skill_creator" => Some(&self.utility),
            "chat" => Some(&self.chat),
            "vision" => Some(&self.vision),
            _ => None,
        }
    }

    /// Set model for a role name
    pub fn set(&mut self, role: &str, model: String) -> bool {
        match role.to_lowercase().as_str() {
            "orchestrator" => { self.orchestrator = model; true }
            "code" | "coder" => { self.code = model; true }
            "research" | "researcher" => { self.research = model; true }
            "reasoning" => { self.reasoning = model; true }
            // Consolidated into utility
            "utility" | "analysis" | "analyst" | "file" | "filesystem" | "general" | "explore" | "explorer" | "skill-creator" | "skill_creator" => { self.utility = model; true }
            "chat" => { self.chat = model; true }
            "vision" => { self.vision = model; true }
            _ => false,
        }
    }

    /// List all available roles
    pub fn roles() -> &'static [&'static str] {
        &["orchestrator", "code", "research", "reasoning", "utility", "chat", "vision"]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenRouterConfig {
    /// API key is stored in keyring, this is just a reference
    #[serde(skip)]
    pub api_key: Option<String>,
    /// Default model for chat
    #[serde(default = "default_model_str")]
    pub default_model: String,
}

fn default_model_str() -> String {
    "anthropic/claude-3.5-sonnet".to_string()
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            default_model: default_model_str(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Daily spending limit in USD
    #[serde(default = "default_daily_limit")]
    pub daily_limit: f64,
    /// Monthly spending limit in USD
    #[serde(default = "default_monthly_limit")]
    pub monthly_limit: f64,
    /// Current day's spending
    #[serde(default)]
    pub current_day_spend: f64,
    /// Current month's spending
    #[serde(default)]
    pub current_month_spend: f64,
}

fn default_daily_limit() -> f64 {
    1.0
}

fn default_monthly_limit() -> f64 {
    10.0
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            daily_limit: default_daily_limit(),
            monthly_limit: default_monthly_limit(),
            current_day_spend: 0.0,
            current_month_spend: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Allowed directories for file operations
    #[serde(default)]
    pub allowed_directories: Vec<PathBuf>,
    /// Require approval for shell commands
    #[serde(default = "default_true")]
    pub require_command_approval: bool,
    /// Sandbox enabled
    #[serde(default = "default_true")]
    pub sandbox_enabled: bool,
    /// Require HTTPS for API authentication
    #[serde(default = "default_true")]
    pub require_https: bool,
}

fn default_true() -> bool {
    true
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            allowed_directories: vec![],
            require_command_approval: true,
            sandbox_enabled: true,
            require_https: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// JWT secret key (auto-generated if not set)
    pub jwt_secret: Option<String>,
    /// Access token expiration (minutes)
    #[serde(default = "default_token_expiry")]
    pub access_token_expiry_minutes: i64,
    /// Refresh token expiration (days)
    #[serde(default = "default_refresh_expiry")]
    pub refresh_token_expiry_days: i64,
    /// Maximum failed login attempts
    #[serde(default = "default_max_attempts")]
    pub max_login_attempts: u32,
    /// Lockout duration after failed attempts (minutes)
    #[serde(default = "default_lockout_duration")]
    pub lockout_duration_minutes: i64,
}

fn default_token_expiry() -> i64 {
    60
}

fn default_refresh_expiry() -> i64 {
    7
}

fn default_max_attempts() -> u32 {
    5
}

fn default_lockout_duration() -> i64 {
    30
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwt_secret: None,
            access_token_expiry_minutes: default_token_expiry(),
            refresh_token_expiry_days: default_refresh_expiry(),
            max_login_attempts: default_max_attempts(),
            lockout_duration_minutes: default_lockout_duration(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            openrouter: OpenRouterConfig::default(),
            models: ModelsConfig::default(),
            budget: BudgetConfig::default(),
            security: SecurityConfig::default(),
            auth: AuthConfig::default(),
            failover: Default::default(),
            gateway: Default::default(),
        }
    }
}

impl Config {
    /// Load configuration from file
    pub fn load() -> Result<Self> {
        let config_path = config_path()?;

        if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path)
                .context("Failed to read config file")?;
            let config: Config = toml::from_str(&contents)
                .context("Failed to parse config file")?;
            Ok(config)
        } else {
            let config = Config::default();
            config.save()?;
            Ok(config)
        }
    }

    /// Save configuration to file
    pub fn save(&self) -> Result<()> {
        let config_path = config_path()?;
        let parent = config_path.parent()
            .context("Config path has no parent")?;

        std::fs::create_dir_all(parent)
            .context("Failed to create config directory")?;

        let contents = toml::to_string_pretty(self)
            .context("Failed to serialize config")?;

        std::fs::write(&config_path, contents)
            .context("Failed to write config file")?;

        Ok(())
    }

    /// Generate and save JWT secret if not exists
    pub fn ensure_jwt_secret(&mut self) -> Result<String> {
        if let Some(secret) = &self.auth.jwt_secret {
            return Ok(secret.clone());
        }

        let secret = crate::server::auth::generate_jwt_secret();
        self.auth.jwt_secret = Some(secret.clone());
        self.save()?;
        Ok(secret)
    }
}

/// Get the configuration file path
pub fn config_path() -> Result<PathBuf> {
    let base = directories::ProjectDirs::from("com", "my-agent", "my-agent")
        .context("Failed to get project directories")?;
    Ok(base.config_dir().join("config.toml"))
}

/// Get the data directory path
pub fn data_dir() -> Result<PathBuf> {
    let base = directories::ProjectDirs::from("com", "my-agent", "my-agent")
        .context("Failed to get project directories")?;
    Ok(base.data_dir().to_path_buf())
}

/// Show current configuration
pub fn show_config() -> Result<()> {
    let config = Config::load()?;

    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║                    Model Configuration                     ║");
    println!("╠════════════════════════════════════════════════════════════╣");
    println!("║ Role            │ Model                                    ║");
    println!("╠═════════════════╪══════════════════════════════════════════╣");
    println!("║ {:<15} │ {:<40} ║", "orchestrator", config.models.orchestrator);
    println!("║ {:<15} │ {:<40} ║", "code", config.models.code);
    println!("║ {:<15} │ {:<40} ║", "research", config.models.research);
    println!("║ {:<15} │ {:<40} ║", "reasoning", config.models.reasoning);
    println!("║ {:<15} │ {:<40} ║", "utility", config.models.utility);
    println!("║ {:<15} │ {:<40} ║", "chat", config.models.chat);
    println!("║ {:<15} │ {:<40} ║", "vision", config.models.vision);
    println!("╚═════════════════╧══════════════════════════════════════════╝");

    println!("\n📝 Note: 'utility' role handles: analysis, file ops, exploration, general, skill-creation");
    println!("\n📊 Features:");
    println!("  ✓ JWT Authentication: {}", config.auth.jwt_secret.as_ref().map(|_| "Configured").unwrap_or("Not configured"));
    println!("  ✓ Budget Tracking: Daily ${}, Monthly ${}", config.budget.daily_limit, config.budget.monthly_limit);
    println!("  ✓ Command Approval: {}", if config.security.require_command_approval { "Required" } else { "Optional" });
    println!("  ✓ Sandbox: {}", if config.security.sandbox_enabled { "Enabled" } else { "Disabled" });

    println!("\n💡 Use 'my-agent config --set-model <role> <model>' to change a model");
    println!("   Available roles: {}", ModelsConfig::roles().join(", "));

    Ok(())
}

/// Set API key
pub fn set_api_key(key: &str) -> Result<()> {
    crate::security::keyring::set_api_key(key)?;
    println!("API key stored securely.");
    Ok(())
}

/// Set daily budget limit
pub fn set_daily_limit(limit: f64) -> Result<()> {
    let mut config = Config::load()?;
    config.budget.daily_limit = limit;
    config.save()?;
    println!("Daily budget limit set to ${}", limit);
    Ok(())
}

/// Set monthly budget limit
pub fn set_monthly_limit(limit: f64) -> Result<()> {
    let mut config = Config::load()?;
    config.budget.monthly_limit = limit;
    config.save()?;
    println!("Monthly budget limit set to ${}", limit);
    Ok(())
}

/// Set allowed directory
pub fn add_allowed_directory(path: &str) -> Result<()> {
    let mut config = Config::load()?;
    let path = PathBuf::from(path);
    if !path.exists() {
        anyhow::bail!("Directory does not exist: {}", path.display());
    }
    config.security.allowed_directories.push(path);
    config.save()?;
    println!("Added allowed directory");
    Ok(())
}

/// Set command approval requirement
pub fn set_command_approval(required: bool) -> Result<()> {
    let mut config = Config::load()?;
    config.security.require_command_approval = required;
    config.save()?;
    println!("Command approval {}", if required { "enabled" } else { "disabled" });
    Ok(())
}

/// Set sandbox mode
pub fn set_sandbox(enabled: bool) -> Result<()> {
    let mut config = Config::load()?;
    config.security.sandbox_enabled = enabled;
    config.save()?;
    println!("Sandbox {}", if enabled { "enabled" } else { "disabled" });
    Ok(())
}

/// Generate new JWT secret
pub fn rotate_jwt_secret() -> Result<()> {
    let mut config = Config::load()?;
    let new_secret = crate::server::auth::generate_jwt_secret();
    config.auth.jwt_secret = Some(new_secret);
    config.save()?;
    println!("JWT secret rotated. All existing tokens are now invalid.");
    Ok(())
}

/// Reset configuration to defaults
pub fn reset_config() -> Result<()> {
    let config = Config::default();
    config.save()?;
    println!("Configuration reset to defaults.");
    Ok(())
}

/// Set model for a specific role
pub fn set_model(role: &str, model: &str) -> Result<()> {
    let mut config = Config::load()?;

    if !config.models.set(role, model.to_string()) {
        anyhow::bail!("Unknown role '{}'. Available roles: {}", role, ModelsConfig::roles().join(", "));
    }

    config.save()?;
    println!("✅ Model for '{}' set to: {}", role, model);
    Ok(())
}

/// Get model for a specific role
pub fn get_model(role: &str) -> Result<()> {
    let config = Config::load()?;

    match config.models.get(role) {
        Some(model) => println!("Model for '{}': {}", role, model),
        None => anyhow::bail!("Unknown role '{}'. Available roles: {}", role, ModelsConfig::roles().join(", ")),
    }

    Ok(())
}

/// List all model assignments
pub fn list_models() -> Result<()> {
    let config = Config::load()?;

    println!("Model Assignments:");
    println!("  orchestrator:    {}", config.models.orchestrator);
    println!("  code:            {}", config.models.code);
    println!("  research:        {}", config.models.research);
    println!("  reasoning:       {}", config.models.reasoning);
    println!("  utility:         {} (analysis, file, explorer, general, skill-creator)", config.models.utility);
    println!("  chat:            {}", config.models.chat);
    println!("  vision:          {}", config.models.vision);

    Ok(())
}

/// Get default configuration as TOML string
pub fn default_config_toml() -> String {
    let config = Config::default();
    toml::to_string_pretty(&config).unwrap_or_else(|_| "# Default configuration\n".to_string())
}