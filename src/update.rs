use crate::daemon::DaemonManager;
use crate::state::AppState;
use anyhow::{anyhow, Result};
use chrono::Utc;
use colored::Colorize;
use serde::Deserialize;
use std::fs;
use std::process::Command;
use std::time::Duration;

const GITHUB_REPO: &str = "kvn1351/ai-limit-ping";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let clean = s.trim_start_matches('v').trim();
    let parts: Vec<&str> = clean.split('.').collect();
    if parts.len() >= 3 {
        let maj = parts[0].parse().ok()?;
        let min = parts[1].parse().ok()?;
        let patch = parts[2].split('-').next()?.parse().ok()?;
        Some((maj, min, patch))
    } else {
        None
    }
}

pub fn is_newer(remote: &str, current: &str) -> bool {
    match (parse_semver(remote), parse_semver(current)) {
        (Some((r_maj, r_min, r_pat)), Some((c_maj, c_min, c_pat))) => {
            (r_maj, r_min, r_pat) > (c_maj, c_min, c_pat)
        }
        _ => false,
    }
}

/// Checks GitHub for a newer version.
/// Uses local cache if checked within the last 1 hour to keep CLI instant.
pub async fn check_for_updates() -> Option<String> {
    let mut state = AppState::load();

    // 1. Check if cache is fresh (< 1 hour)
    if let Some(last_check) = state.last_update_check {
        let age_hours = (Utc::now() - last_check).num_hours();
        if age_hours < 1 {
            if let Some(cached_ver) = &state.latest_known_version {
                if is_newer(cached_ver, CURRENT_VERSION) {
                    return Some(cached_ver.clone());
                } else {
                    return None;
                }
            }
        }
    }

    // 2. Fetch latest release from GitHub API with short 1.5s timeout
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(1500))
        .user_agent(format!("alp/{}", CURRENT_VERSION))
        .build()
        .ok()?;

    let url = format!("https://api.github.com/repos/{}/releases/latest", GITHUB_REPO);
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }

    let release: GithubRelease = resp.json().await.ok()?;
    let latest_tag = release.tag_name.trim_start_matches('v').to_string();

    // Cache to state
    state.last_update_check = Some(Utc::now());
    state.latest_known_version = Some(latest_tag.clone());
    let _ = state.save();

    if is_newer(&latest_tag, CURRENT_VERSION) {
        Some(latest_tag)
    } else {
        None
    }
}

/// Displays an unobtrusive update notice after running a command
pub fn print_update_notice(latest_ver: &str) {
    println!(
        "\n{} A new version of alp is available: {} → {}\n  Run {} to upgrade.",
        "Tip:".yellow().bold(),
        CURRENT_VERSION.dimmed(),
        latest_ver.green().bold(),
        "alp update".cyan().bold()
    );
}

/// Handles the `alp update` command to download and self-update the binary
pub async fn handle_update() -> Result<()> {
    println!("{} Checking for updates from GitHub...", "→".cyan().bold());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(format!("alp/{}", CURRENT_VERSION))
        .build()?;

    let url = format!("https://api.github.com/repos/{}/releases/latest", GITHUB_REPO);
    let resp = client.get(&url).send().await?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        println!(
            "{} alp is already up to date ({})",
            "✓".green().bold(),
            CURRENT_VERSION
        );
        return Ok(());
    }

    if !resp.status().is_success() {
        return Err(anyhow!("Failed to query GitHub releases: HTTP {}", resp.status()));
    }

    let release: GithubRelease = resp.json().await?;
    let latest_ver = release.tag_name.trim_start_matches('v').to_string();

    if !is_newer(&latest_ver, CURRENT_VERSION) {
        println!(
            "{} alp is already up to date ({})",
            "✓".green().bold(),
            CURRENT_VERSION
        );
        return Ok(());
    }

    println!(
        "{} Updating alp: {} → {}...",
        "→".cyan().bold(),
        CURRENT_VERSION.dimmed(),
        latest_ver.green().bold()
    );

    // Identify matching platform target
    let target_prefix = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "alp-aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "alp-x86_64-apple-darwin"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "alp-x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "alp-x86_64-pc-windows-msvc"
    } else {
        return Err(anyhow!("Unsupported platform architecture for precompiled binary"));
    };

    let asset = release
        .assets
        .iter()
        .find(|a| a.name.starts_with(target_prefix) && !a.name.ends_with(".sha256"))
        .ok_or_else(|| anyhow!("No compatible release binary asset found for {}", target_prefix))?;

    println!("  Downloading {}...", asset.name);
    let bytes = client.get(&asset.browser_download_url).send().await?.bytes().await?;

    // Create temp directory for extraction
    let temp_dir = std::env::temp_dir().join(format!("alp_update_{}", std::process::id()));
    fs::create_dir_all(&temp_dir)?;

    let archive_path = temp_dir.join(&asset.name);
    fs::write(&archive_path, &bytes)?;

    println!("  Extracting binary...");
    #[cfg(unix)]
    {
        let status = Command::new("tar")
            .args(["-xzf", archive_path.to_str().unwrap(), "-C", temp_dir.to_str().unwrap()])
            .status()?;
        if !status.success() {
            let _ = fs::remove_dir_all(&temp_dir);
            return Err(anyhow!("Failed to extract release archive using tar"));
        }
    }

    #[cfg(windows)]
    {
        let status = Command::new("tar")
            .args(["-xzf", archive_path.to_str().unwrap(), "-C", temp_dir.to_str().unwrap()])
            .status()?;
        if !status.success() {
            let _ = fs::remove_dir_all(&temp_dir);
            return Err(anyhow!("Failed to extract release archive using tar"));
        }
    }

    let bin_name = if cfg!(windows) { "alp.exe" } else { "alp" };
    let new_bin = temp_dir.join(bin_name);

    if !new_bin.exists() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(anyhow!("Extracted binary '{}' not found in archive", bin_name));
    }

    let current_exe = std::env::current_exe()?;
    println!("  Installing to {}...", current_exe.display());

    // Replace binary safely
    let backup_path = current_exe.with_extension("old");
    let _ = fs::rename(&current_exe, &backup_path);
    if let Err(e) = fs::copy(&new_bin, &current_exe) {
        // Rollback on failure
        let _ = fs::rename(&backup_path, &current_exe);
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(anyhow!("Failed to install new binary: {}", e));
    }
    let _ = fs::remove_file(&backup_path);
    let _ = fs::remove_dir_all(&temp_dir);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&current_exe, fs::Permissions::from_mode(0o755));
    }

    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("xattr")
            .args(["-d", "com.apple.quarantine", current_exe.to_str().unwrap()])
            .status();
        let _ = Command::new("codesign")
            .args(["-s", "-", "-f", current_exe.to_str().unwrap()])
            .status();
    }

    // Update local state cache
    let mut state = AppState::load();
    state.last_update_check = Some(Utc::now());
    state.latest_known_version = Some(latest_ver.clone());
    let _ = state.save();

    // Restart background daemon if active
    if DaemonManager::state() == crate::daemon::DaemonState::Active {
        let _ = DaemonManager::stop();
        let _ = DaemonManager::start();
    }

    println!("\n{} Successfully updated alp to v{}!", "✓".green().bold(), latest_ver);
    Ok(())
}
