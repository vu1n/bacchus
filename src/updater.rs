//! Self-update functionality for bacchus
//!
//! Handles checking for updates and downloading new versions atomically.

use serde::{Deserialize, Serialize};
use std::env;
use std::fs;

const GITHUB_REPO: &str = "vu1n/bacchus";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    body: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] ureq::Error),

    #[error("Failed to parse JSON: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("No binary found for platform {0}-{1}")]
    BinaryNotFound(String, String),

    #[error("Already on latest version: {0}")]
    AlreadyLatest(String),
}

/// Result type for update operations
pub type UpdateResult<T> = Result<T, UpdateError>;

/// Check if a newer version is available
pub fn check_for_updates() -> UpdateResult<UpdateInfo> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", GITHUB_REPO);

    let release: GitHubRelease = ureq::get(&url)
        .set("User-Agent", "bacchus")
        .call()?
        .into_json()?;

    let latest_version = release.tag_name.trim_start_matches('v');

    // Compare versions (simple semver comparison)
    if version_compare::compare_versions(CURRENT_VERSION, latest_version)
        .map_or(false, |v| v >= std::cmp::Ordering::Equal)
    {
        return Ok(UpdateInfo {
            current_version: CURRENT_VERSION.to_string(),
            latest_version: latest_version.to_string(),
            update_available: false,
            release_url: release.html_url,
            release_notes: release.body,
        });
    }

    Ok(UpdateInfo {
        current_version: CURRENT_VERSION.to_string(),
        latest_version: latest_version.to_string(),
        update_available: true,
        release_url: release.html_url,
        release_notes: release.body,
    })
}

/// Information about available updates
#[derive(Debug, Serialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_url: String,
    pub release_notes: Option<String>,
}

/// Perform self-update to the latest version
pub fn self_update() -> UpdateResult<String> {
    println!("Checking for updates...");
    let info = check_for_updates()?;

    if !info.update_available {
        println!("Already on latest version: {}", info.current_version);
        return Err(UpdateError::AlreadyLatest(info.current_version));
    }

    println!("Update available: {} -> {}", info.current_version, info.latest_version);
    println!("Release notes:");
    if let Some(notes) = &info.release_notes {
        println!("{}", notes);
    }
    println!("\nDownloading update from: {}", info.release_url);

    // Detect platform
    let (os, arch) = detect_platform();
    let binary_name = format!("bacchus-{}-{}", os, arch);
    let download_url = format!(
        "https://github.com/{}/releases/download/v{}/{}",
        GITHUB_REPO, info.latest_version, binary_name
    );

    // Get current binary path
    let current_exe = env::current_exe()?;
    let install_dir = current_exe
        .parent()
        .ok_or_else(|| UpdateError::IoError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Cannot determine install directory",
        )))?;

    let temp_path = install_dir.join("bacchus.tmp");

    // Download to temporary file
    println!("Downloading from: {}", download_url);
    let response = ureq::get(&download_url)
        .set("User-Agent", "bacchus")
        .call()?;

    let mut reader = response.into_reader();
    let mut temp_file = fs::File::create(&temp_path)?;
    std::io::copy(&mut reader, &mut temp_file)?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&temp_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&temp_path, perms)?;
    }

    // Atomic replace
    println!("Installing update...");
    fs::rename(&temp_path, &current_exe)?;

    println!("Updated to version {}!", info.latest_version);
    println!("Run 'bacchus --version' to verify.");

    Ok(info.latest_version)
}

/// Detect the current platform (OS and architecture)
fn detect_platform() -> (String, String) {
    let os = env::consts::OS;
    let arch = env::consts::ARCH;

    let os_name = match os {
        "linux" => "linux",
        "macos" => "darwin",
        _ => os,
    };

    let arch_name = match arch {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        "arm" => "aarch64",
        _ => arch,
    };

    (os_name.to_string(), arch_name.to_string())
}

/// Simple version comparison for semver strings
mod version_compare {
    pub fn compare_versions(a: &str, b: &str) -> Option<std::cmp::Ordering> {
        let a_parts: Vec<u32> = a
            .split('.')
            .map(|s| s.parse().ok())
            .collect::<Option<_>>()?;
        let b_parts: Vec<u32> = b
            .split('.')
            .map(|s| s.parse().ok())
            .collect::<Option<_>>()?;

        Some(a_parts.cmp(&b_parts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_compare() {
        assert_eq!(
            version_compare::compare_versions("0.1.0", "0.1.1"),
            Some(std::cmp::Ordering::Less)
        );
        assert_eq!(
            version_compare::compare_versions("0.2.0", "0.1.9"),
            Some(std::cmp::Ordering::Greater)
        );
        assert_eq!(
            version_compare::compare_versions("1.0.0", "1.0.0"),
            Some(std::cmp::Ordering::Equal)
        );
    }
}
