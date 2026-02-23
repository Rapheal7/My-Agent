//! Diagnostic reports for the doctor command

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Severity level for diagnostic issues
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
pub enum Severity {
    /// Informational, no action needed
    Info,
    /// Minor issue, should be addressed eventually
    Warning,
    /// Important issue, should be addressed soon
    Error,
    /// Critical issue, must be addressed immediately
    Critical,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Info => write!(f, "ℹ️  INFO"),
            Severity::Warning => write!(f, "⚠️  WARN"),
            Severity::Error => write!(f, "❌ ERROR"),
            Severity::Critical => write!(f, "🔥 CRITICAL"),
        }
    }
}

/// A single diagnostic check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    /// Name of the check
    pub name: String,
    /// Check category
    pub category: CheckCategory,
    /// Severity of the result
    pub severity: Severity,
    /// Whether the check passed
    pub passed: bool,
    /// Human-readable message
    pub message: String,
    /// Optional fix suggestion
    pub fix: Option<String>,
    /// Whether this can be auto-fixed
    pub auto_fixable: bool,
    /// Additional details
    pub details: Vec<String>,
}

impl CheckResult {
    /// Create a passing check
    pub fn pass(name: impl Into<String>, category: CheckCategory, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            category,
            severity: Severity::Info,
            passed: true,
            message: message.into(),
            fix: None,
            auto_fixable: false,
            details: vec![],
        }
    }

    /// Create a failing check
    pub fn fail(
        name: impl Into<String>,
        category: CheckCategory,
        severity: Severity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            category,
            severity,
            passed: false,
            message: message.into(),
            fix: None,
            auto_fixable: false,
            details: vec![],
        }
    }

    /// Add a fix suggestion
    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    /// Mark as auto-fixable
    pub fn auto_fix(mut self) -> Self {
        self.auto_fixable = true;
        self
    }

    /// Add details
    pub fn with_details(mut self, details: Vec<String>) -> Self {
        self.details = details;
        self
    }
}

/// Category of diagnostic check
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CheckCategory {
    /// Security vulnerabilities
    Security,
    /// Configuration issues
    Configuration,
    /// Dependency issues
    Dependencies,
    /// System requirements
    System,
    /// Network connectivity
    Network,
    /// File system
    FileSystem,
    /// API keys and authentication
    Authentication,
}

impl fmt::Display for CheckCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CheckCategory::Security => write!(f, "Security"),
            CheckCategory::Configuration => write!(f, "Configuration"),
            CheckCategory::Dependencies => write!(f, "Dependencies"),
            CheckCategory::System => write!(f, "System"),
            CheckCategory::Network => write!(f, "Network"),
            CheckCategory::FileSystem => write!(f, "FileSystem"),
            CheckCategory::Authentication => write!(f, "Authentication"),
        }
    }
}

/// A vulnerability advisory from RustSec
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vulnerability {
    /// Advisory ID (e.g., "RUSTSEC-2024-0001")
    pub id: String,
    /// Affected package
    pub package: String,
    /// Vulnerable versions
    pub vulnerable_versions: String,
    /// Patched versions
    pub patched_versions: Option<String>,
    /// Severity (if known)
    pub severity: Option<String>,
    /// Description
    pub description: String,
    /// CVE ID if available
    pub cve: Option<String>,
    /// Whether there's a fix available
    pub fix_available: bool,
}

impl fmt::Display for Vulnerability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} in {}", self.id, self.package)?;
        if let Some(ref cve) = self.cve {
            write!(f, " ({})", cve)?;
        }
        Ok(())
    }
}

/// Outdated dependency information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutdatedDependency {
    /// Package name
    pub name: String,
    /// Current version
    pub current: String,
    /// Latest version
    pub latest: String,
    /// Whether it's a security update
    pub security_update: bool,
}

/// Update information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    /// Current version
    pub current_version: String,
    /// Latest available version
    pub latest_version: String,
    /// Whether an update is available
    pub update_available: bool,
    /// Release notes URL
    pub release_url: Option<String>,
    /// Download URL for the binary
    pub download_url: Option<String>,
}

/// Complete diagnostic report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticReport {
    /// When the report was generated
    pub timestamp: DateTime<Utc>,
    /// Agent version
    pub version: String,
    /// All check results
    pub checks: Vec<CheckResult>,
    /// Found vulnerabilities
    pub vulnerabilities: Vec<Vulnerability>,
    /// Outdated dependencies
    pub outdated_dependencies: Vec<OutdatedDependency>,
    /// Update information
    pub update_info: Option<UpdateInfo>,
    /// Overall health status
    pub healthy: bool,
    /// Summary message
    pub summary: String,
}

impl DiagnosticReport {
    /// Create a new empty report
    pub fn new(version: &str) -> Self {
        Self {
            timestamp: Utc::now(),
            version: version.to_string(),
            checks: Vec::new(),
            vulnerabilities: Vec::new(),
            outdated_dependencies: Vec::new(),
            update_info: None,
            healthy: true,
            summary: String::new(),
        }
    }

    /// Add a check result
    pub fn add_check(&mut self, check: CheckResult) {
        if !check.passed {
            self.healthy = false;
        }
        self.checks.push(check);
    }

    /// Add a vulnerability
    pub fn add_vulnerability(&mut self, vuln: Vulnerability) {
        self.healthy = false;
        self.vulnerabilities.push(vuln);
    }

    /// Add an outdated dependency
    pub fn add_outdated(&mut self, dep: OutdatedDependency) {
        if dep.security_update {
            self.healthy = false;
        }
        self.outdated_dependencies.push(dep);
    }

    /// Set update info
    pub fn set_update_info(&mut self, info: UpdateInfo) {
        self.update_info = Some(info);
    }

    /// Finalize the report and generate summary
    pub fn finalize(&mut self) {
        let passed = self.checks.iter().filter(|c| c.passed).count();
        let failed = self.checks.len() - passed;
        let vulns = self.vulnerabilities.len();
        let outdated = self.outdated_dependencies.len();

        let mut summary_parts = vec![];

        if failed == 0 && vulns == 0 {
            summary_parts.push("✅ All checks passed".to_string());
        } else {
            if failed > 0 {
                summary_parts.push(format!("{} check(s) failed", failed));
            }
            if vulns > 0 {
                summary_parts.push(format!("{} vulnerability(ies) found", vulns));
            }
        }

        if outdated > 0 {
            summary_parts.push(format!("{} outdated dependencies", outdated));
        }

        if let Some(ref info) = self.update_info {
            if info.update_available {
                summary_parts.push(format!(
                    "Update available: {} → {}",
                    info.current_version, info.latest_version
                ));
            }
        }

        self.summary = summary_parts.join(". ");
    }

    /// Get auto-fixable issues
    pub fn auto_fixable_issues(&self) -> Vec<&CheckResult> {
        self.checks
            .iter()
            .filter(|c| !c.passed && c.auto_fixable)
            .collect()
    }

    /// Count issues by severity
    pub fn count_by_severity(&self, severity: Severity) -> usize {
        self.checks
            .iter()
            .filter(|c| !c.passed && c.severity == severity)
            .count()
    }
}

impl fmt::Display for DiagnosticReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "\n╔══════════════════════════════════════════════════════════════╗")?;
        writeln!(f, "║           MY-AGENT DIAGNOSTIC REPORT                         ║")?;
        writeln!(f, "╚══════════════════════════════════════════════════════════════╝")?;
        writeln!(f)?;
        writeln!(f, "📅 Generated: {}", self.timestamp.format("%Y-%m-%d %H:%M:%S UTC"))?;
        writeln!(f, "📦 Version: {}", self.version)?;
        writeln!(f)?;

        // Checks by category
        let mut categories: Vec<CheckCategory> = self.checks.iter().map(|c| c.category).collect();
        categories.sort();
        categories.dedup();

        for category in categories {
            writeln!(f, "┌─ {} ─────────────────────────────────────────", category)?;
            let checks: Vec<_> = self.checks.iter().filter(|c| c.category == category).collect();

            for check in checks {
                let status = if check.passed { "✓" } else { "✗" };
                writeln!(f, "│ {} {} [{}] {}", status, check.severity, check.name, check.message)?;

                if !check.details.is_empty() {
                    for detail in &check.details {
                        writeln!(f, "│    • {}", detail)?;
                    }
                }

                if !check.passed {
                    if let Some(ref fix) = check.fix {
                        writeln!(f, "│    💡 Fix: {}", fix)?;
                    }
                    if check.auto_fixable {
                        writeln!(f, "│    🔧 Auto-fix available")?;
                    }
                }
            }
            writeln!(f, "└──────────────────────────────────────────────────────────────")?;
            writeln!(f)?;
        }

        // Vulnerabilities
        if !self.vulnerabilities.is_empty() {
            writeln!(f, "┌─ SECURITY VULNERABILITIES ───────────────────────────────────")?;
            for vuln in &self.vulnerabilities {
                writeln!(f, "│ 🔥 {} in {}", vuln.id, vuln.package)?;
                if let Some(ref cve) = vuln.cve {
                    writeln!(f, "│    CVE: {}", cve)?;
                }
                writeln!(f, "│    {}", vuln.description)?;
                if let Some(ref patched) = vuln.patched_versions {
                    writeln!(f, "│    Patched: {}", patched)?;
                }
                writeln!(f)?;
            }
            writeln!(f, "└──────────────────────────────────────────────────────────────")?;
            writeln!(f)?;
        }

        // Outdated dependencies
        if !self.outdated_dependencies.is_empty() {
            writeln!(f, "┌─ OUTDATED DEPENDENCIES ──────────────────────────────────────")?;
            for dep in &self.outdated_dependencies {
                let security = if dep.security_update { " ⚠️ SECURITY" } else { "" };
                writeln!(f, "│ {} {} → {}{}", dep.name, dep.current, dep.latest, security)?;
            }
            writeln!(f, "└──────────────────────────────────────────────────────────────")?;
            writeln!(f)?;
        }

        // Update info
        if let Some(ref info) = self.update_info {
            writeln!(f, "┌─ UPDATE STATUS ──────────────────────────────────────────────")?;
            if info.update_available {
                writeln!(f, "│ 🆙 Update available: {} → {}", info.current_version, info.latest_version)?;
                if let Some(ref url) = info.release_url {
                    writeln!(f, "│    Release notes: {}", url)?;
                }
            } else {
                writeln!(f, "│ ✅ Running latest version: {}", info.current_version)?;
            }
            writeln!(f, "└──────────────────────────────────────────────────────────────")?;
            writeln!(f)?;
        }

        // Summary
        writeln!(f, "══════════════════════════════════════════════════════════════")?;
        if self.healthy {
            writeln!(f, "✅ STATUS: HEALTHY")?;
        } else {
            writeln!(f, "⚠️  STATUS: ISSUES FOUND")?;
        }
        writeln!(f, "📝 {}", self.summary)?;
        writeln!(f, "══════════════════════════════════════════════════════════════")?;

        Ok(())
    }
}

/// Result of an auto-fix operation
#[derive(Debug, Clone)]
pub struct FixResult {
    /// Check that was fixed
    pub check_name: String,
    /// Whether the fix succeeded
    pub success: bool,
    /// Message describing what was done
    pub message: String,
}
