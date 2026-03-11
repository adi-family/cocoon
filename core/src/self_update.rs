use lib_console_output::{out_info, out_success, KeyValue, Renderable};
use semver::Version;
use std::path::PathBuf;

use lib_env_parse::{env_opt, env_vars};

env_vars! {
    Home => "HOME",
}

const REPO_OWNER: &str = "adi-family";
const REPO_NAME: &str = "cocoon";
const DOCKER_IMAGE: &str = "docker-registry.the-ihor.com/cocoon";

#[derive(Debug, Clone)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_notes: Option<String>,
}

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

pub fn check_for_updates() -> Result<UpdateCheckResult, String> {
    let current_version = env!("CARGO_PKG_VERSION");
    let (latest_version, release_notes) = fetch_latest_version()?;

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

pub fn download_latest_binary(install_dir: &PathBuf) -> Result<String, String> {
    use self_update::backends::github::Update;
    use self_update::cargo_crate_version;

    let current_version = cargo_crate_version!();
    let target = get_target_triple();

    out_info!("  Current version: {}", current_version);
    out_info!("  Target: {}", target);
    out_info!("  Checking for updates...");

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

pub mod docker {
    use lib_console_output::out_info;
    use super::DOCKER_IMAGE;

    pub fn pull_latest_image(tag: &str) -> Result<bool, String> {
        let image = format!("{}:{}", DOCKER_IMAGE, tag);

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

        out_info!("  Pulling {}...", image);

        let output = std::process::Command::new("docker")
            .args(["pull", &image])
            .status()
            .map_err(|e| format!("Failed to pull image: {}", e))?;

        if !output.success() {
            return Err("Failed to pull image".to_string());
        }

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

        let updated = match (before_digest, after_digest) {
            (Some(before), Some(after)) => before != after,
            (None, Some(_)) => true, // New image pulled
            _ => true,               // Assume updated if we can't determine
        };

        Ok(updated)
    }

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

    pub fn recreate_container(container_name: &str, tag: &str) -> Result<String, String> {
        let image = format!("{}:{}", DOCKER_IMAGE, tag);

        out_info!("  Saving container configuration...");
        let env_vars = get_container_env(container_name)?;
        let volumes = get_container_volumes(container_name)?;

        out_info!("  Stopping old container...");
        let _ = std::process::Command::new("docker")
            .args(["stop", container_name])
            .status();

        out_info!("  Removing old container...");
        let _ = std::process::Command::new("docker")
            .args(["rm", container_name])
            .status();

        out_info!("  Creating new container...");
        let mut cmd = std::process::Command::new("docker");
        cmd.args([
            "run",
            "-d",
            "--restart",
            "unless-stopped",
            "--name",
            container_name,
        ]);

        for (key, value) in &env_vars {
            // Skip internal Docker env vars
            if key == "PATH" || key == "HOME" || key.starts_with("HOSTNAME") {
                continue;
            }
            cmd.args(["-e", &format!("{}={}", key, value)]);
        }

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

    pub fn check_for_updates(tag: &str) -> Result<(bool, String), String> {
        let image = format!("{}:{}", DOCKER_IMAGE, tag);

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

pub mod machine {
    use super::*;
    use std::path::Path;

    pub fn get_install_dir() -> Result<PathBuf, String> {
        // Try to get from current exe location
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(parent) = exe_path.parent() {
                return Ok(parent.to_path_buf());
            }
        }

        // Fallback to ~/.local/bin
        let home = env_opt(EnvVar::Home.as_str()).ok_or_else(|| "HOME not set".to_string())?;
        Ok(PathBuf::from(format!("{}/.local/bin", home)))
    }

    pub fn update_binary() -> Result<String, String> {
        let install_dir = get_install_dir()?;

        out_info!("  Install directory: {}", install_dir.display());

        if !install_dir.exists() {
            std::fs::create_dir_all(&install_dir)
                .map_err(|e| format!("Failed to create install directory: {}", e))?;
        }

        download_latest_binary(&install_dir)
    }

    pub fn update_and_restart() -> Result<String, String> {
        out_info!("Updating cocoon binary...");
        let update_result = update_binary()?;

        if update_result.contains("Already up to date") {
            return Ok(update_result);
        }

        out_info!("Restarting service...");

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
                let home = env_opt(EnvVar::Home.as_str()).ok_or_else(|| "HOME not set".to_string())?;
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

pub fn format_check_result(result: &UpdateCheckResult) -> String {
    KeyValue::new()
        .entry("Current version", &result.current_version)
        .entry("Latest version", &result.latest_version)
        .print();

    if result.update_available {
        out_success!("Update available!");

        if let Some(ref notes) = result.release_notes {
            out_info!("Release notes:");
            let truncated: String = notes.chars().take(500).collect();
            let suffix = if notes.len() > 500 { "..." } else { "" };
            out_info!("{}{}", truncated, suffix);
        }

        let hint = "Run 'adi cocoon update <name>' to update.".to_string();
        out_info!("{}", hint);
        hint
    } else {
        let msg = "You are running the latest version.".to_string();
        out_success!("{}", msg);
        msg
    }
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
