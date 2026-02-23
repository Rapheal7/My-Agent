//! Health checks for the doctor command

use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;

use super::report::{CheckResult, CheckCategory, Severity};

/// Run all health checks
pub async fn run_all_checks() -> Vec<CheckResult> {
    let mut results = Vec::new();

    // Configuration checks
    results.extend(check_config().await);

    // Authentication checks
    results.extend(check_api_keys().await);

    // System checks
    results.extend(check_system().await);

    // Network checks
    results.extend(check_network().await);

    // File system checks
    results.extend(check_filesystem().await);

    results
}

/// Check configuration files
pub async fn check_config() -> Vec<CheckResult> {
    let mut results = Vec::new();

    // Check config directory exists
    let config_dir = dirs::config_dir()
        .map(|p| p.join("my-agent"))
        .unwrap_or_else(|| PathBuf::from(".my-agent"));

    if config_dir.exists() {
        results.push(CheckResult::pass(
            "config_directory",
            CheckCategory::Configuration,
            "Configuration directory exists",
        ));
    } else {
        results.push(
            CheckResult::fail(
                "config_directory",
                CheckCategory::Configuration,
                Severity::Warning,
                "Configuration directory not found",
            )
            .with_fix("Run 'my-agent config --show' to create default configuration")
            .auto_fix(),
        );
    }

    // Check config file
    let config_file = config_dir.join("config.toml");
    if config_file.exists() {
        match std::fs::read_to_string(&config_file) {
            Ok(content) => {
                // Validate TOML syntax
                match content.parse::<toml::Value>() {
                    Ok(_) => {
                        results.push(CheckResult::pass(
                            "config_file",
                            CheckCategory::Configuration,
                            "Configuration file is valid TOML",
                        ));
                    }
                    Err(e) => {
                        results.push(
                            CheckResult::fail(
                                "config_file",
                                CheckCategory::Configuration,
                                Severity::Error,
                                format!("Configuration file has invalid TOML: {}", e),
                            )
                            .with_fix("Fix the TOML syntax in the configuration file"),
                        );
                    }
                }
            }
            Err(e) => {
                results.push(
                    CheckResult::fail(
                        "config_file",
                        CheckCategory::Configuration,
                        Severity::Error,
                        format!("Cannot read configuration file: {}", e),
                    )
                    .with_fix("Check file permissions"),
                );
            }
        }
    } else {
        results.push(
            CheckResult::fail(
                "config_file",
                CheckCategory::Configuration,
                Severity::Info,
                "Configuration file not found (will use defaults)",
            )
            .with_fix("Run 'my-agent config' to create configuration"),
        );
    }

    // Check personality file
    let personality_file = config_dir.join("personality.json");
    if personality_file.exists() {
        match std::fs::read_to_string(&personality_file) {
            Ok(content) => {
                match serde_json::from_str::<serde_json::Value>(&content) {
                    Ok(_) => {
                        results.push(CheckResult::pass(
                            "personality_file",
                            CheckCategory::Configuration,
                            "Personality file is valid JSON",
                        ));
                    }
                    Err(e) => {
                        results.push(
                            CheckResult::fail(
                                "personality_file",
                                CheckCategory::Configuration,
                                Severity::Warning,
                                format!("Personality file has invalid JSON: {}", e),
                            )
                            .with_fix("Fix the JSON syntax or delete the file to use defaults"),
                        );
                    }
                }
            }
            Err(e) => {
                results.push(
                    CheckResult::fail(
                        "personality_file",
                        CheckCategory::Configuration,
                        Severity::Warning,
                        format!("Cannot read personality file: {}", e),
                    ),
                );
            }
        }
    }

    results
}

/// Check API keys and authentication
pub async fn check_api_keys() -> Vec<CheckResult> {
    let mut results = Vec::new();

    // Check OpenRouter API key in keyring
    match crate::security::get_api_key() {
        Ok(key) => {
            if key.is_empty() {
                results.push(
                    CheckResult::fail(
                        "openrouter_api_key",
                        CheckCategory::Authentication,
                        Severity::Error,
                        "OpenRouter API key is empty",
                    )
                    .with_fix("Set your API key with: my-agent config --set-api-key YOUR_KEY"),
                );
            } else if key.len() < 10 {
                results.push(
                    CheckResult::fail(
                        "openrouter_api_key",
                        CheckCategory::Authentication,
                        Severity::Warning,
                        "OpenRouter API key appears to be invalid (too short)",
                    )
                    .with_fix("Set a valid API key with: my-agent config --set-api-key YOUR_KEY"),
                );
            } else {
                results.push(CheckResult::pass(
                    "openrouter_api_key",
                    CheckCategory::Authentication,
                    "OpenRouter API key is configured",
                ));

                // Optionally validate the key by making a test request
                if let Err(e) = validate_api_key(&key).await {
                    results.push(
                        CheckResult::fail(
                            "openrouter_api_key_valid",
                            CheckCategory::Authentication,
                            Severity::Warning,
                            format!("API key validation failed: {}", e),
                        )
                        .with_fix("Check if your API key is still valid"),
                    );
                }
            }
        }
        Err(e) => {
            results.push(
                CheckResult::fail(
                    "openrouter_api_key",
                    CheckCategory::Authentication,
                    Severity::Error,
                    format!("Cannot access API key from keyring: {}", e),
                )
                .with_fix("Set your API key with: my-agent config --set-api-key YOUR_KEY"),
            );
        }
    }

    // Check HuggingFace API key (optional, for voice features)
    match crate::security::get_hf_api_key() {
        Ok(key) if !key.is_empty() => {
            results.push(CheckResult::pass(
                "huggingface_api_key",
                CheckCategory::Authentication,
                "HuggingFace API key is configured (voice features enabled)",
            ));
        }
        _ => {
            results.push(
                CheckResult::fail(
                    "huggingface_api_key",
                    CheckCategory::Authentication,
                    Severity::Info,
                    "HuggingFace API key not configured (voice features may be limited)",
                )
                .with_fix("Set with: my-agent config --set-hf-api-key YOUR_KEY"),
            );
        }
    }

    results
}

/// Validate API key by making a test request
async fn validate_api_key(key: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .get("https://openrouter.ai/api/v1/models")
        .header("Authorization", format!("Bearer {}", key))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await?;

    if response.status().is_success() {
        Ok(())
    } else {
        anyhow::bail!("API returned status {}", response.status());
    }
}

/// Check system requirements
pub async fn check_system() -> Vec<CheckResult> {
    let mut results = Vec::new();

    // Check Rust version
    if let Ok(output) = Command::new("rustc").arg("--version").output() {
        let version = String::from_utf8_lossy(&output.stdout);
        results.push(CheckResult::pass(
            "rust_version",
            CheckCategory::System,
            format!("Rust installed: {}", version.trim()),
        ));
    } else {
        results.push(
            CheckResult::fail(
                "rust_version",
                CheckCategory::System,
                Severity::Warning,
                "Rust not found in PATH",
            )
            .with_fix("Install Rust from https://rustup.rs"),
        );
    }

    // Check cargo
    if let Ok(output) = Command::new("cargo").arg("--version").output() {
        let version = String::from_utf8_lossy(&output.stdout);
        results.push(CheckResult::pass(
            "cargo_version",
            CheckCategory::System,
            format!("Cargo installed: {}", version.trim()),
        ));
    } else {
        results.push(
            CheckResult::fail(
                "cargo_version",
                CheckCategory::System,
                Severity::Warning,
                "Cargo not found in PATH",
            )
            .with_fix("Install Rust from https://rustup.rs"),
        );
    }

    // Check available memory (approximate)
    #[cfg(target_os = "linux")]
    {
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            for line in meminfo.lines() {
                if line.starts_with("MemAvailable:") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(kb) = parts[1].parse::<u64>() {
                            let mb = kb / 1024;
                            if mb > 1024 {
                                results.push(CheckResult::pass(
                                    "memory",
                                    CheckCategory::System,
                                    format!("Available memory: {} MB", mb),
                                ));
                            } else {
                                results.push(
                                    CheckResult::fail(
                                        "memory",
                                        CheckCategory::System,
                                        Severity::Warning,
                                        format!("Low available memory: {} MB", mb),
                                    )
                                    .with_fix("Close other applications to free memory"),
                                );
                            }
                        }
                    }
                    break;
                }
            }
        }
    }

    // Check disk space for data directory
    let data_dir = dirs::data_local_dir()
        .map(|p| p.join("my-agent"))
        .unwrap_or_else(|| PathBuf::from("."));

    if let Ok(metadata) = std::fs::metadata(data_dir.parent().unwrap_or(&PathBuf::from("."))) {
        results.push(CheckResult::pass(
            "data_directory",
            CheckCategory::System,
            "Data directory accessible",
        ));
    }

    results
}

/// Check network connectivity
pub async fn check_network() -> Vec<CheckResult> {
    let mut results = Vec::new();

    // Check OpenRouter connectivity
    match reqwest::get("https://openrouter.ai/api/v1/models")
        .await
    {
        Ok(response) => {
            if response.status().is_success() {
                results.push(CheckResult::pass(
                    "openrouter_connectivity",
                    CheckCategory::Network,
                    "OpenRouter API is accessible",
                ));
            } else {
                results.push(
                    CheckResult::fail(
                        "openrouter_connectivity",
                        CheckCategory::Network,
                        Severity::Warning,
                        format!("OpenRouter API returned status {}", response.status()),
                    ),
                );
            }
        }
        Err(e) => {
            results.push(
                CheckResult::fail(
                    "openrouter_connectivity",
                    CheckCategory::Network,
                    Severity::Error,
                    format!("Cannot reach OpenRouter API: {}", e),
                )
                .with_fix("Check your internet connection"),
            );
        }
    }

    // Check GitHub connectivity (for updates)
    match reqwest::Client::new()
        .head("https://github.com")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(_) => {
            results.push(CheckResult::pass(
                "github_connectivity",
                CheckCategory::Network,
                "GitHub is accessible (updates available)",
            ));
        }
        Err(e) => {
            results.push(
                CheckResult::fail(
                    "github_connectivity",
                    CheckCategory::Network,
                    Severity::Info,
                    format!("Cannot reach GitHub: {} (updates unavailable)", e),
                ),
            );
        }
    }

    results
}

/// Check file system permissions and storage
pub async fn check_filesystem() -> Vec<CheckResult> {
    let mut results = Vec::new();

    // Check config directory permissions
    let config_dir = dirs::config_dir()
        .map(|p| p.join("my-agent"))
        .unwrap_or_else(|| PathBuf::from(".my-agent"));

    if config_dir.exists() {
        // Try to write a test file
        let test_file = config_dir.join(".write_test");
        match std::fs::write(&test_file, "test") {
            Ok(_) => {
                std::fs::remove_file(&test_file).ok();
                results.push(CheckResult::pass(
                    "config_write_permission",
                    CheckCategory::FileSystem,
                    "Configuration directory is writable",
                ));
            }
            Err(e) => {
                results.push(
                    CheckResult::fail(
                        "config_write_permission",
                        CheckCategory::FileSystem,
                        Severity::Error,
                        format!("Cannot write to config directory: {}", e),
                    )
                    .with_fix("Check directory permissions"),
                );
            }
        }
    }

    // Check data directory
    let data_dir = dirs::data_local_dir()
        .map(|p| p.join("my-agent"))
        .unwrap_or_else(|| PathBuf::from("."));

    if !data_dir.exists() {
        match std::fs::create_dir_all(&data_dir) {
            Ok(_) => {
                results.push(CheckResult::pass(
                    "data_directory_create",
                    CheckCategory::FileSystem,
                    "Data directory created",
                ));
            }
            Err(e) => {
                results.push(
                    CheckResult::fail(
                        "data_directory",
                        CheckCategory::FileSystem,
                        Severity::Error,
                        format!("Cannot create data directory: {}", e),
                    )
                    .with_fix("Check parent directory permissions"),
                );
            }
        }
    } else {
        // Check memory database
        let db_path = data_dir.join("memory.db");
        if db_path.exists() {
            match rusqlite::Connection::open(&db_path) {
                Ok(_) => {
                    results.push(CheckResult::pass(
                        "memory_database",
                        CheckCategory::FileSystem,
                        "Memory database is accessible",
                    ));
                }
                Err(e) => {
                    results.push(
                        CheckResult::fail(
                            "memory_database",
                            CheckCategory::FileSystem,
                            Severity::Warning,
                            format!("Cannot open memory database: {}", e),
                        )
                        .with_fix("Database may be corrupted. Delete it to recreate."),
                    );
                }
            }
        }
    }

    // Check temp directory
    let temp_dir = std::env::temp_dir();
    let test_file = temp_dir.join(".my-agent-write-test");
    match std::fs::write(&test_file, "test") {
        Ok(_) => {
            std::fs::remove_file(&test_file).ok();
            results.push(CheckResult::pass(
                "temp_directory",
                CheckCategory::FileSystem,
                "Temp directory is writable",
            ));
        }
        Err(e) => {
            results.push(
                CheckResult::fail(
                    "temp_directory",
                    CheckCategory::FileSystem,
                    Severity::Warning,
                    format!("Cannot write to temp directory: {}", e),
                ),
            );
        }
    }

    results
}

/// Attempt to auto-fix an issue
pub async fn auto_fix(check: &CheckResult) -> Result<String> {
    match check.name.as_str() {
        "config_directory" => {
            let config_dir = dirs::config_dir()
                .map(|p| p.join("my-agent"))
                .ok_or_else(|| anyhow::anyhow!("Cannot determine config directory"))?;
            std::fs::create_dir_all(&config_dir)?;
            Ok("Created configuration directory".to_string())
        }
        "config_file" => {
            let config_dir = dirs::config_dir()
                .map(|p| p.join("my-agent"))
                .ok_or_else(|| anyhow::anyhow!("Cannot determine config directory"))?;
            let config_file = config_dir.join("config.toml");
            let default_config = crate::config::default_config_toml();
            std::fs::write(&config_file, default_config)?;
            Ok("Created default configuration file".to_string())
        }
        "data_directory" => {
            let data_dir = dirs::data_local_dir()
                .map(|p| p.join("my-agent"))
                .ok_or_else(|| anyhow::anyhow!("Cannot determine data directory"))?;
            std::fs::create_dir_all(&data_dir)?;
            Ok("Created data directory".to_string())
        }
        _ => {
            anyhow::bail!("No auto-fix available for: {}", check.name)
        }
    }
}
