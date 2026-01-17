//! Self-update functionality for cocoon
//!
//! Provides the ability to check for updates and update cocoon across different runtimes:
//! - Docker: Pull latest image and recreate container
//! - Machine: Download new binary and restart service

use semver::Version;
use std::path::PathBuf;

const REPO_OWNER: &str = "adi-family";
const REPO_NAME: &str = "cocoon";
const DOCKER_IMAGE: &str = "docker-registry.the-ihor.com/cocoon";

/// Result of checking for updates
#[derive(Debug, Clone)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_notes: Option<String>,
}

/// Get the target triple for the current platform
pub fn get_target_triple() -> String {
    let os = if cfg!(target_os = "linux") {
        "unknown-linux-musl"
    } else if cfg!(target_os = "macos") {
        "apple-darwin"
    } else if cfg!(target_os = "windows") {
        "pc-windows-msvc"
    } else {
        "unknown"
    };

    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };

    format!("{}-{}", arch, os)
}

/// Fetch the latest release version from GitHub
pub fn fetch_latest_version() -> Result<(String, Option<String>), String> {
    use self_update::backends::github::ReleaseList;

    let releases = ReleaseList::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .build()
        .map_err(|e| format!("Failed to configure release list: {}", e))?
        .fetch()
        .map_err(|e| format!("Failed to fetch releases: {}", e))?;

    let latest = releases
        .first()
        .ok_or_else(|| "No releases found".to_string())?;

    let version = latest.version.trim_start_matches('v').to_string();
    let notes = latest.body.clone();

    Ok((version, notes))
}

/// Check for available updates (for Machine runtime - binary)
pub fn check_for_updates() -> Result<UpdateCheckResult, String> {
    let current_version = env!("CARGO_PKG_VERSION");
    let (latest_version, release_notes) = fetch_latest_version()?;

    // Parse versions for comparison
    let current = Version::parse(current_version).map_err(|e| {
        format!(
            "Failed to parse current version '{}': {}",
            current_version, e
        )
    })?;
    let latest = Version::parse(&latest_version)
        .map_err(|e| format!("Failed to parse latest version '{}': {}", latest_version, e))?;

    let update_available = latest > current;

    Ok(UpdateCheckResult {
        current_version: current_version.to_string(),
        latest_version,
        update_available,
        release_notes,
    })
}

/// Download and extract the latest binary for Machine runtime
pub fn download_latest_binary(install_dir: &PathBuf) -> Result<String, String> {
    use self_update::backends::github::Update;
    use self_update::cargo_crate_version;

    let current_version = cargo_crate_version!();
    let target = get_target_triple();

    println!("  Current version: {}", current_version);
    println!("  Target: {}", target);
    println!("  Checking for updates...");

    let status = Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name("cocoon")
        .current_version(current_version)
        .target(&target)
        .bin_install_path(install_dir)
        .no_confirm(true)
        .show_download_progress(true)
        .show_output(true)
        .build()
        .map_err(|e| format!("Failed to configure updater: {}", e))?
        .update()
        .map_err(|e| format!("Update failed: {}", e))?;

    match status {
        self_update::Status::UpToDate(v) => Ok(format!("Already up to date (version {})", v)),
        self_update::Status::Updated(v) => Ok(format!("Updated to version {}", v)),
    }
}

/// Docker-specific update functions
pub mod docker {
    use super::DOCKER_IMAGE;

    /// Pull the latest image and return whether it was updated
    pub fn pull_latest_image(tag: &str) -> Result<bool, String> {
        let image = format!("{}:{}", DOCKER_IMAGE, tag);

        // Get current digest before pull
        let before_digest = std::process::Command::new("docker")
            .args(["images", "--digests", "--format", "{{.Digest}}", &image])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            });

        println!("  Pulling {}...", image);

        let output = std::process::Command::new("docker")
            .args(["pull", &image])
            .status()
            .map_err(|e| format!("Failed to pull image: {}", e))?;

        if !output.success() {
            return Err("Failed to pull image".to_string());
        }

        // Get digest after pull
        let after_digest = std::process::Command::new("docker")
            .args(["images", "--digests", "--format", "{{.Digest}}", &image])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            });

        // Check if image was updated
        let updated = match (before_digest, after_digest) {
            (Some(before), Some(after)) => before != after,
            (None, Some(_)) => true, // New image pulled
            _ => true,               // Assume updated if we can't determine
        };

        Ok(updated)
    }

    /// Get container environment variables
    pub fn get_container_env(container_name: &str) -> Result<Vec<(String, String)>, String> {
        let output = std::process::Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{range .Config.Env}}{{println .}}{{end}}",
                container_name,
            ])
            .output()
            .map_err(|e| format!("Failed to inspect container: {}", e))?;

        if !output.status.success() {
            return Err(format!("Container '{}' not found", container_name));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let env_vars: Vec<(String, String)> = stdout
            .lines()
            .filter(|line| !line.is_empty())
            .filter_map(|line| {
                let parts: Vec<&str> = line.splitn(2, '=').collect();
                if parts.len() == 2 {
                    Some((parts[0].to_string(), parts[1].to_string()))
                } else {
                    None
                }
            })
            .collect();

        Ok(env_vars)
    }

    /// Get container volumes
    pub fn get_container_volumes(container_name: &str) -> Result<Vec<String>, String> {
        let output = std::process::Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{range .Mounts}}{{.Source}}:{{.Destination}} {{end}}",
                container_name,
            ])
            .output()
            .map_err(|e| format!("Failed to inspect container: {}", e))?;

        if !output.status.success() {
            return Err(format!("Container '{}' not found", container_name));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let volumes: Vec<String> = stdout
            .split_whitespace()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        Ok(volumes)
    }

    /// Recreate a container with the latest image
    pub fn recreate_container(container_name: &str, tag: &str) -> Result<String, String> {
        let image = format!("{}:{}", DOCKER_IMAGE, tag);

        // Get container config before removing
        println!("  Saving container configuration...");
        let env_vars = get_container_env(container_name)?;
        let volumes = get_container_volumes(container_name)?;

        // Stop and remove old container
        println!("  Stopping old container...");
        let _ = std::process::Command::new("docker")
            .args(["stop", container_name])
            .status();

        println!("  Removing old container...");
        let _ = std::process::Command::new("docker")
            .args(["rm", container_name])
            .status();

        // Create new container with same config
        println!("  Creating new container...");
        let mut cmd = std::process::Command::new("docker");
        cmd.args([
            "run",
            "-d",
            "--restart",
            "unless-stopped",
            "--name",
            container_name,
        ]);

        // Add environment variables
        for (key, value) in &env_vars {
            // Skip internal Docker env vars
            if key == "PATH" || key == "HOME" || key.starts_with("HOSTNAME") {
                continue;
            }
            cmd.args(["-e", &format!("{}={}", key, value)]);
        }

        // Add volumes
        for vol in &volumes {
            cmd.args(["-v", vol]);
        }

        // Add host mapping for .local domains
        for (key, value) in &env_vars {
            if key == "SIGNALING_SERVER_URL" {
                if let Ok(url) = url::Url::parse(value) {
                    if let Some(host) = url.host_str() {
                        if host.ends_with(".local") {
                            cmd.args(["--add-host", &format!("{}:host-gateway", host)]);
                        }
                    }
                }
            }
        }

        cmd.arg(&image);

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to create container: {}", e))?;

        if output.status.success() {
            let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(format!(
                "Container recreated: {}",
                &container_id[..12.min(container_id.len())]
            ))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("Failed to create container: {}", stderr))
        }
    }

    /// Check if a newer image is available
    pub fn check_for_updates(tag: &str) -> Result<(bool, String), String> {
        let image = format!("{}:{}", DOCKER_IMAGE, tag);

        // Get local digest
        let local_output = std::process::Command::new("docker")
            .args(["images", "--digests", "--format", "{{.Digest}}", &image])
            .output()
            .map_err(|e| format!("Failed to check local image: {}", e))?;

        let local_digest = if local_output.status.success() {
            String::from_utf8_lossy(&local_output.stdout)
                .trim()
                .to_string()
        } else {
            String::new()
        };

        // Check remote digest (this requires pulling manifest)
        // For now, we'll just report the local digest and suggest pulling
        if local_digest.is_empty() {
            Ok((true, "No local image found".to_string()))
        } else {
            Ok((
                false,
                format!(
                    "Local digest: {}",
                    &local_digest[..19.min(local_digest.len())]
                ),
            ))
        }
    }
}

/// Machine-specific update functions
pub mod machine {
    use super::*;
    use std::path::Path;

    /// Get the install directory for the cocoon binary
    pub fn get_install_dir() -> Result<PathBuf, String> {
        // Try to get from current exe location
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(parent) = exe_path.parent() {
                return Ok(parent.to_path_buf());
            }
        }

        // Fallback to ~/.local/bin
        let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
        Ok(PathBuf::from(format!("{}/.local/bin", home)))
    }

    /// Download and install the latest binary
    pub fn update_binary() -> Result<String, String> {
        let install_dir = get_install_dir()?;

        println!("  Install directory: {}", install_dir.display());

        // Ensure install directory exists
        if !install_dir.exists() {
            std::fs::create_dir_all(&install_dir)
                .map_err(|e| format!("Failed to create install directory: {}", e))?;
        }

        download_latest_binary(&install_dir)
    }

    /// Update and restart the service
    pub fn update_and_restart() -> Result<String, String> {
        println!("Updating cocoon binary...");
        let update_result = update_binary()?;

        if update_result.contains("Already up to date") {
            return Ok(update_result);
        }

        println!("\nRestarting service...");

        let os = detect_os();
        match os {
            "linux" => {
                let output = std::process::Command::new("systemctl")
                    .args(["--user", "restart", "cocoon"])
                    .status()
                    .map_err(|e| format!("Failed to restart service: {}", e))?;

                if output.success() {
                    Ok(format!(
                        "{}\nService restarted successfully.",
                        update_result
                    ))
                } else {
                    Ok(format!(
                        "{}\nWarning: Service restart may have failed. Check status with: systemctl --user status cocoon",
                        update_result
                    ))
                }
            }
            "macos" => {
                let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
                let plist = format!("{}/Library/LaunchAgents/com.adi.cocoon.plist", home);

                if Path::new(&plist).exists() {
                    let _ = std::process::Command::new("launchctl")
                        .args(["unload", &plist])
                        .status();

                    std::thread::sleep(std::time::Duration::from_secs(1));

                    let _ = std::process::Command::new("launchctl")
                        .args(["load", &plist])
                        .status();

                    Ok(format!(
                        "{}\nService restarted successfully.",
                        update_result
                    ))
                } else {
                    Ok(format!(
                        "{}\nNote: No service installed. Start manually if needed.",
                        update_result
                    ))
                }
            }
            _ => Ok(format!(
                "{}\nNote: Cannot restart service on this OS.",
                update_result
            )),
        }
    }

    fn detect_os() -> &'static str {
        #[cfg(target_os = "linux")]
        return "linux";

        #[cfg(target_os = "macos")]
        return "macos";

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        return "unknown";
    }
}

/// Format update check result for display
pub fn format_check_result(result: &UpdateCheckResult) -> String {
    let mut output = String::new();

    output.push_str(&format!("Current version: {}\n", result.current_version));
    output.push_str(&format!("Latest version:  {}\n", result.latest_version));
    output.push('\n');

    if result.update_available {
        output.push_str("Update available!\n\n");

        if let Some(ref notes) = result.release_notes {
            output.push_str("Release notes:\n");
            let truncated: String = notes.chars().take(500).collect();
            output.push_str(&truncated);
            if notes.len() > 500 {
                output.push_str("...\n");
            }
            output.push('\n');
        }

        output.push_str("\nRun 'adi cocoon update <name>' to update.\n");
    } else {
        output.push_str("You are running the latest version.\n");
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_triple() {
        let target = get_target_triple();
        assert!(!target.is_empty());
        assert!(target.contains('-'));
    }
}
