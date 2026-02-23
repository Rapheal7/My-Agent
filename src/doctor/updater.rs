//! Self-update logic for my-agent

use anyhow::{Result, Context, bail};
use serde::{Deserialize, Serialize};
use std::env::consts::{ARCH, OS};
use std::path::PathBuf;
use std::process::Command;
use tracing::{info, warn};

use super::report::UpdateInfo;

/// GitHub repository for releases
const GITHUB_REPO: &str = "rapheal/my-agent"; // TODO: Update with actual repo
const GITHUB_API_URL: &str = "https://api.github.com/repos";

/// GitHub release information
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    html_url: String,
    assets: Vec<GitHubAsset>,
    draft: bool,
    prerelease: bool,
}

/// GitHub release asset
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    content_type: String,
}

/// Self-updater
pub struct Updater {
    /// Current version
    current_version: String,
    /// GitHub repository
    repo: String,
}

impl Updater {
    /// Create a new updater
    pub fn new() -> Self {
        let current_version = env!("CARGO_PKG_VERSION").to_string();
        Self {
            current_version,
            repo: GITHUB_REPO.to_string(),
        }
    }

    /// Set custom repository
    pub fn with_repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Check for updates
    pub async fn check_update(&self) -> Result<UpdateInfo> {
        info!("Checking for updates...");

        let release = self.fetch_latest_release().await?;

        let latest_version = release
            .tag_name
            .trim_start_matches('v')
            .to_string();

        let update_available = self.compare_versions(&latest_version, &self.current_version)?;

        // Find appropriate asset for current platform
        let download_url = self.find_download_url(&release);

        Ok(UpdateInfo {
            current_version: self.current_version.clone(),
            latest_version,
            update_available,
            release_url: Some(release.html_url),
            download_url,
        })
    }

    /// Fetch latest release from GitHub
    async fn fetch_latest_release(&self) -> Result<GitHubRelease> {
        let url = format!("{}/{}/releases/latest", GITHUB_API_URL, self.repo);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("my-agent-updater")
            .build()?;

        let response = client
            .get(&url)
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .await
            .context("Failed to fetch release information from GitHub")?;

        if response.status() == 404 {
            bail!("Repository or releases not found: {}", self.repo);
        }

        if !response.status().is_success() {
            bail!("GitHub API error: {}", response.status());
        }

        let release: GitHubRelease = response
            .json()
            .await
            .context("Failed to parse GitHub response")?;

        Ok(release)
    }

    /// Compare versions, returns true if latest > current
    fn compare_versions(&self, latest: &str, current: &str) -> Result<bool> {
        let latest_parts: Vec<u32> = latest
            .split('.')
            .filter_map(|s| s.parse().ok())
            .collect();

        let current_parts: Vec<u32> = current
            .split('.')
            .filter_map(|s| s.parse().ok())
            .collect();

        if latest_parts.is_empty() || current_parts.is_empty() {
            bail!("Invalid version format");
        }

        // Compare major, minor, patch
        for i in 0..std::cmp::max(latest_parts.len(), current_parts.len()) {
            let latest_part = latest_parts.get(i).unwrap_or(&0);
            let current_part = current_parts.get(i).unwrap_or(&0);

            if latest_part > current_part {
                return Ok(true);
            } else if latest_part < current_part {
                return Ok(false);
            }
        }

        Ok(false)
    }

    /// Find download URL for current platform
    fn find_download_url(&self, release: &GitHubRelease) -> Option<String> {
        // Determine target triple
        let target = self.get_target_triple();

        // Find matching asset
        for asset in &release.assets {
            let name_lower = asset.name.to_lowercase();

            // Check if asset matches our platform
            if name_lower.contains(&target.to_lowercase()) {
                return Some(asset.browser_download_url.clone());
            }

            // Check common patterns
            if name_lower.contains(&format!("{}-{}", OS, ARCH).to_lowercase()) {
                return Some(asset.browser_download_url.clone());
            }

            if name_lower.contains(&target.replace('-', "_").to_lowercase()) {
                return Some(asset.browser_download_url.clone());
            }
        }

        // Fallback: look for any binary for our OS
        let os_lower = OS.to_lowercase();
        for asset in &release.assets {
            let name_lower = asset.name.to_lowercase();
            if name_lower.contains(&os_lower) {
                return Some(asset.browser_download_url.clone());
            }
        }

        None
    }

    /// Get target triple for current platform
    fn get_target_triple(&self) -> String {
        let arch = match ARCH {
            "x86_64" => "x86_64",
            "aarch64" => "aarch64",
            "arm" => "arm",
            _ => ARCH,
        };

        let os = match OS {
            "linux" => "unknown-linux-gnu",
            "macos" => "apple-darwin",
            "windows" => "pc-windows-msvc",
            _ => OS,
        };

        format!("{}-{}", arch, os)
    }

    /// Download and install update
    pub async fn install_update(&self, download_url: &str) -> Result<()> {
        info!("Downloading update from: {}", download_url);

        // Determine current executable path
        let current_exe = std::env::current_exe()
            .context("Failed to get current executable path")?;

        let exe_dir = current_exe
            .parent()
            .context("Failed to get executable directory")?;

        // Download to temp file
        let temp_dir = std::env::temp_dir();
        let filename = download_url
            .rsplit('/')
            .next()
            .unwrap_or("my-agent-update");

        let temp_file = temp_dir.join(filename);

        info!("Downloading to: {:?}", temp_file);

        // Download the file
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300)) // 5 minutes for large files
            .build()?;

        let response = client
            .get(download_url)
            .send()
            .await
            .context("Failed to download update")?;

        if !response.status().is_success() {
            bail!("Download failed: {}", response.status());
        }

        let bytes = response
            .bytes()
            .await
            .context("Failed to read download")?;

        std::fs::write(&temp_file, &bytes)
            .context("Failed to write update file")?;

        // Handle different file types
        if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
            self.extract_and_install_tarball(&temp_file, exe_dir, &current_exe)?;
        } else if filename.ends_with(".zip") {
            self.extract_and_install_zip(&temp_file, exe_dir, &current_exe)?;
        } else {
            // Assume it's a binary
            self.install_binary(&temp_file, &current_exe)?;
        }

        info!("Update installed successfully!");
        println!("✓ Update installed. Restart my-agent to use the new version.");

        Ok(())
    }

    /// Extract tarball and install
    fn extract_and_install_tarball(
        &self,
        tarball: &PathBuf,
        dest_dir: &std::path::Path,
        current_exe: &std::path::Path,
    ) -> Result<()> {
        use std::io::Read;

        // Extract tarball
        let file = std::fs::File::open(tarball)?;
        let gz_decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(gz_decoder);

        let temp_extract = tarball.parent().unwrap().join("extracted");
        std::fs::create_dir_all(&temp_extract)?;
        archive.unpack(&temp_extract)?;

        // Find the binary
        let binary_name = if OS == "windows" {
            "my-agent.exe"
        } else {
            "my-agent"
        };

        let extracted_binary = self.find_binary_in_dir(&temp_extract, binary_name)?;

        // Install
        self.install_binary(&extracted_binary, current_exe)?;

        // Cleanup
        std::fs::remove_dir_all(&temp_extract).ok();
        std::fs::remove_file(tarball).ok();

        Ok(())
    }

    /// Extract zip and install (for Windows)
    fn extract_and_install_zip(
        &self,
        zip_file: &PathBuf,
        dest_dir: &std::path::Path,
        current_exe: &std::path::Path,
    ) -> Result<()> {
        let temp_extract = zip_file.parent().unwrap().join("extracted");
        std::fs::create_dir_all(&temp_extract)?;

        // Use system unzip or a Rust zip library
        #[cfg(target_os = "windows")]
        {
            Command::new("powershell")
                .args([
                    "-Command",
                    &format!(
                        "Expand-Archive -Path '{}' -DestinationPath '{}'",
                        zip_file.display(),
                        temp_extract.display()
                    ),
                ])
                .status()?;
        }

        #[cfg(not(target_os = "windows"))]
        {
            Command::new("unzip")
                .args(["-o", "-q"])
                .arg(zip_file)
                .args(["-d", &temp_extract.to_string_lossy()])
                .status()?;
        }

        // Find and install binary
        let binary_name = if OS == "windows" {
            "my-agent.exe"
        } else {
            "my-agent"
        };

        let extracted_binary = self.find_binary_in_dir(&temp_extract, binary_name)?;
        self.install_binary(&extracted_binary, current_exe)?;

        // Cleanup
        std::fs::remove_dir_all(&temp_extract).ok();
        std::fs::remove_file(zip_file).ok();

        Ok(())
    }

    /// Find binary in extracted directory
    fn find_binary_in_dir(&self, dir: &std::path::Path, name: &str) -> Result<PathBuf> {
        for entry in walkdir::WalkDir::new(dir) {
            let entry = entry?;
            if entry.file_name().to_string_lossy() == name {
                return Ok(entry.path().to_path_buf());
            }
        }
        bail!("Binary not found in extracted archive: {}", name);
    }

    /// Install a binary file
    fn install_binary(&self, source: &std::path::Path, target: &std::path::Path) -> Result<()> {
        // Backup old binary
        let backup = target.with_extension("backup");
        if target.exists() {
            std::fs::rename(target, &backup)
                .context("Failed to backup old binary")?;
        }

        // Copy new binary
        std::fs::copy(source, target)
            .context("Failed to copy new binary")?;

        // Make executable (Unix)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o755))
                .context("Failed to set executable permissions")?;
        }

        // Remove backup
        if backup.exists() {
            std::fs::remove_file(&backup).ok();
        }

        Ok(())
    }
}

impl Default for Updater {
    fn default() -> Self {
        Self::new()
    }
}

/// Check for updates
pub async fn check_for_updates() -> Result<UpdateInfo> {
    let updater = Updater::new();
    updater.check_update().await
}

/// Perform self-update
pub async fn self_update() -> Result<()> {
    let updater = Updater::new();
    let info = updater.check_update().await?;

    if !info.update_available {
        println!("Already running the latest version: {}", info.current_version);
        return Ok(());
    }

    println!("Updating from {} to {}...", info.current_version, info.latest_version);

    if let Some(ref download_url) = info.download_url {
        updater.install_update(download_url).await?;
    } else {
        println!("No download available for your platform.");
        if let Some(ref release_url) = info.release_url {
            println!("Download manually from: {}", release_url);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_updater_creation() {
        let updater = Updater::new();
        assert!(!updater.current_version.is_empty());
    }

    #[test]
    fn test_version_comparison() {
        let updater = Updater::new();

        assert!(updater.compare_versions("2.0.0", "1.0.0").unwrap());
        assert!(updater.compare_versions("1.1.0", "1.0.0").unwrap());
        assert!(updater.compare_versions("1.0.1", "1.0.0").unwrap());
        assert!(!updater.compare_versions("1.0.0", "1.0.0").unwrap());
        assert!(!updater.compare_versions("1.0.0", "2.0.0").unwrap());
    }

    #[test]
    fn test_target_triple() {
        let updater = Updater::new();
        let triple = updater.get_target_triple();

        // Should contain architecture and OS info
        assert!(triple.contains(ARCH) || triple.contains("x86_64") || triple.contains("aarch64"));
    }
}