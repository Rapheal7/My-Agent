//! Doctor module - self-healing and diagnostics
//!
//! Provides:
//! - Health checks (configuration, API keys, system, network, filesystem)
//! - CVE/vulnerability scanning using RustSec
//! - Self-update from GitHub releases
//! - Auto-fix for common issues

pub mod checks;
pub mod cve;
pub mod updater;
pub mod report;

use anyhow::Result;
use tracing::{info, warn};

use report::{DiagnosticReport, CheckResult, FixResult};

/// Run diagnostics
pub async fn run_diagnostics(fix: bool, update: bool) -> Result<()> {
    println!("\n🏥 my-agent Doctor - Self-Healing Diagnostics");
    println!("═══════════════════════════════════════════════════════════\n");

    let mut report = DiagnosticReport::new(env!("CARGO_PKG_VERSION"));

    // Run health checks
    println!("🔍 Running health checks...");
    let check_results = checks::run_all_checks().await;
    for result in check_results {
        // Show progress
        let status = if result.passed { "✓" } else { "✗" };
        println!("  {} {}: {}", status, result.name, result.message);
        report.add_check(result);
    }

    // Check for vulnerabilities
    println!("\n🔒 Checking for security vulnerabilities...");
    match cve::run_audit().await {
        Ok(vulns) => {
            if vulns.is_empty() {
                println!("  ✓ No vulnerabilities found");
            } else {
                println!("  ✗ Found {} vulnerability(ies)", vulns.len());
                for vuln in &vulns {
                    println!("    • {} in {}", vuln.id, vuln.package);
                    report.add_vulnerability(vuln.clone());
                }
            }
        }
        Err(e) => {
            println!("  ⚠️  Could not check vulnerabilities: {}", e);
            println!("     (Install cargo-audit for full scanning: cargo install cargo-audit)");
        }
    }

    // Check for outdated dependencies
    println!("\n📦 Checking for outdated dependencies...");
    match cve::check_outdated().await {
        Ok(outdated) => {
            if outdated.is_empty() {
                println!("  ✓ Dependencies are up to date");
            } else {
                println!("  ⚠️  Found {} outdated package(s)", outdated.len());
                for dep in &outdated {
                    let security = if dep.security_update { " (security)" } else { "" };
                    println!("    • {} {} → {}{}", dep.name, dep.current, dep.latest, security);
                    report.add_outdated(dep.clone());
                }
            }
        }
        Err(e) => {
            println!("  ⚠️  Could not check outdated dependencies: {}", e);
            println!("     (Install cargo-outdated for full checking: cargo install cargo-outdated)");
        }
    }

    // Check for updates
    println!("\n🔄 Checking for updates...");
    match updater::check_for_updates().await {
        Ok(info) => {
            if info.update_available {
                println!("  🆙 Update available: {} → {}", info.current_version, info.latest_version);
                if let Some(ref url) = info.release_url {
                    println!("     Release notes: {}", url);
                }
            } else {
                println!("  ✓ Running latest version: {}", info.current_version);
            }
            report.set_update_info(info);
        }
        Err(e) => {
            println!("  ⚠️  Could not check for updates: {}", e);
        }
    }

    // Finalize report
    report.finalize();

    // Print full report
    println!("{}", report);

    // Auto-fix if requested
    if fix {
        let fixable: Vec<_> = report.auto_fixable_issues().iter().map(|c| (*c).clone()).collect();
        if !fixable.is_empty() {
            println!("\n🔧 Applying auto-fixes...");
            for check in &fixable {
                match checks::auto_fix(check).await {
                    Ok(message) => {
                        println!("  ✓ {}: {}", check.name, message);
                        report.add_check(
                            CheckResult::pass(
                                &check.name,
                                check.category,
                                "Issue auto-fixed",
                            )
                        );
                    }
                    Err(e) => {
                        println!("  ✗ {}: Failed to fix - {}", check.name, e);
                    }
                }
            }
        } else {
            println!("\n💡 No auto-fixable issues found.");
        }
    }

    // Self-update if requested
    if update {
        if let Some(ref info) = report.update_info {
            if info.update_available {
                println!("\n🔄 Performing self-update...");
                match updater::self_update().await {
                    Ok(()) => {
                        println!("  ✓ Update installed successfully!");
                    }
                    Err(e) => {
                        println!("  ✗ Update failed: {}", e);
                        if let Some(ref url) = info.download_url {
                            println!("     Download manually from: {}", url);
                        }
                    }
                }
            } else {
                println!("\n✓ Already running latest version.");
            }
        }
    }

    // Print recommendations
    if !report.healthy {
        println!("\n💡 Recommendations:");
        let mut recommendations = Vec::new();

        for check in &report.checks {
            if !check.passed {
                if let Some(ref fix) = check.fix {
                    recommendations.push(format!("• {} - {}", check.name, fix));
                }
            }
        }

        if !report.vulnerabilities.is_empty() {
            recommendations.push("• Run 'cargo update' to get patched versions".to_string());
        }

        if !report.outdated_dependencies.is_empty() {
            recommendations.push("• Run 'cargo update' to update dependencies".to_string());
        }

        for rec in recommendations {
            println!("  {}", rec);
        }

        // Suggest re-running doctor with --fix
        let fixable_count = report.auto_fixable_issues().len();
        if fixable_count > 0 && !fix {
            println!("\n  Run 'my-agent doctor --fix' to auto-fix {} issue(s)", fixable_count);
        }

        // Suggest updating
        if let Some(ref info) = report.update_info {
            if info.update_available && !update {
                println!("  Run 'my-agent doctor --update' to update to {}", info.latest_version);
            }
        }
    }

    Ok(())
}

/// Run a quick health check (lighter version for startup)
pub async fn quick_health_check() -> Result<bool> {
    let mut healthy = true;

    // Quick API key check
    match crate::security::get_api_key() {
        Ok(key) if !key.is_empty() => {}
        _ => {
            warn!("API key not configured");
            healthy = false;
        }
    }

    // Quick config check
    let config_dir = dirs::config_dir()
        .map(|p| p.join("my-agent"))
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    if !config_dir.exists() {
        info!("Config directory will be created on first use");
    }

    Ok(healthy)
}

/// Fix a specific issue by name
pub async fn fix_issue(issue_name: &str) -> Result<()> {
    let checks = checks::run_all_checks().await;

    for check in checks {
        if check.name == issue_name && check.auto_fixable {
            match checks::auto_fix(&check).await {
                Ok(message) => {
                    println!("✓ Fixed: {}", message);
                    return Ok(());
                }
                Err(e) => {
                    anyhow::bail!("Failed to fix {}: {}", issue_name, e);
                }
            }
        }
    }

    anyhow::bail!("No fixable issue found with name: {}", issue_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_quick_health_check() {
        // This will fail if no API key is set, which is expected in tests
        let _ = quick_health_check().await;
    }
}