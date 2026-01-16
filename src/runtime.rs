//! Runtime abstraction for different cocoon backends.
//!
//! Supports Docker containers and Machine (native systemd/launchd) services.

use std::fmt;

/// Runtime type for a cocoon instance
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeType {
    Docker,
    Machine,
}

impl fmt::Display for RuntimeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeType::Docker => write!(f, "docker"),
            RuntimeType::Machine => write!(f, "machine"),
        }
    }
}

impl RuntimeType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "docker" => Some(RuntimeType::Docker),
            "machine" | "native" | "service" => Some(RuntimeType::Machine),
            _ => None,
        }
    }
}

/// Status of a cocoon instance
#[derive(Debug, Clone)]
pub enum CocoonStatus {
    Running,
    Stopped,
    Restarting,
    Unknown(String),
}

impl fmt::Display for CocoonStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CocoonStatus::Running => write!(f, "running"),
            CocoonStatus::Stopped => write!(f, "stopped"),
            CocoonStatus::Restarting => write!(f, "restarting"),
            CocoonStatus::Unknown(s) => write!(f, "{}", s),
        }
    }
}

/// Information about a cocoon instance
#[derive(Debug, Clone)]
pub struct CocoonInfo {
    pub name: String,
    pub runtime: RuntimeType,
    pub status: CocoonStatus,
    pub created: Option<String>,
    pub image: Option<String>,
}

impl CocoonInfo {
    pub fn status_icon(&self) -> &'static str {
        match self.status {
            CocoonStatus::Running => "●",
            CocoonStatus::Stopped => "○",
            CocoonStatus::Restarting => "◐",
            CocoonStatus::Unknown(_) => "?",
        }
    }

    pub fn status_color(&self) -> &'static str {
        match self.status {
            CocoonStatus::Running => "\x1b[32m",    // green
            CocoonStatus::Stopped => "\x1b[90m",    // gray
            CocoonStatus::Restarting => "\x1b[33m", // yellow
            CocoonStatus::Unknown(_) => "\x1b[31m", // red
        }
    }
}

/// Trait for runtime backends
pub trait Runtime {
    /// List all cocoons for this runtime
    fn list(&self) -> Result<Vec<CocoonInfo>, String>;

    /// Get status of a specific cocoon
    fn status(&self, name: &str) -> Result<CocoonInfo, String>;

    /// Start a cocoon
    fn start(&self, name: &str) -> Result<String, String>;

    /// Stop a cocoon
    fn stop(&self, name: &str) -> Result<String, String>;

    /// Restart a cocoon
    fn restart(&self, name: &str) -> Result<String, String>;

    /// Show logs for a cocoon
    fn logs(&self, name: &str, follow: bool, tail: Option<u32>) -> Result<(), String>;

    /// Remove a cocoon
    fn remove(&self, name: &str, force: bool) -> Result<String, String>;

    /// Check if this runtime is available on the system
    fn is_available(&self) -> bool;

    /// Get runtime type
    fn runtime_type(&self) -> RuntimeType;
}

// === Docker Runtime ===

pub struct DockerRuntime;

impl DockerRuntime {
    pub fn new() -> Self {
        DockerRuntime
    }

    fn parse_status(status_str: &str) -> CocoonStatus {
        let lower = status_str.to_lowercase();
        if lower.contains("up") || lower.contains("running") {
            CocoonStatus::Running
        } else if lower.contains("exited") || lower.contains("stopped") || lower.contains("created")
        {
            CocoonStatus::Stopped
        } else if lower.contains("restarting") {
            CocoonStatus::Restarting
        } else {
            CocoonStatus::Unknown(status_str.to_string())
        }
    }
}

impl Runtime for DockerRuntime {
    fn list(&self) -> Result<Vec<CocoonInfo>, String> {
        let output = std::process::Command::new("docker")
            .args([
                "ps",
                "-a",
                "--filter",
                "name=cocoon-",
                "--format",
                "{{.Names}}\t{{.Status}}\t{{.Image}}\t{{.CreatedAt}}",
            ])
            .output()
            .map_err(|e| format!("Failed to run docker: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Docker error: {}", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut cocoons = Vec::new();

        for line in stdout.lines() {
            if line.trim().is_empty() {
                continue;
            }

            let parts: Vec<&str> = line.split('\t').collect();
            if parts.is_empty() {
                continue;
            }

            let name = parts[0].to_string();
            let status_str = parts.get(1).unwrap_or(&"unknown");
            let image = parts.get(2).map(|s| s.to_string());
            let created = parts.get(3).map(|s| s.to_string());

            cocoons.push(CocoonInfo {
                name,
                runtime: RuntimeType::Docker,
                status: Self::parse_status(status_str),
                created,
                image,
            });
        }

        Ok(cocoons)
    }

    fn status(&self, name: &str) -> Result<CocoonInfo, String> {
        let output = std::process::Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{.State.Status}}\t{{.Config.Image}}\t{{.Created}}",
                name,
            ])
            .output()
            .map_err(|e| format!("Failed to run docker: {}", e))?;

        if !output.status.success() {
            return Err(format!("Container '{}' not found", name));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let parts: Vec<&str> = stdout.split('\t').collect();

        let status_str = parts.first().unwrap_or(&"unknown");
        let image = parts.get(1).map(|s| s.to_string());
        let created = parts.get(2).map(|s| s.to_string());

        Ok(CocoonInfo {
            name: name.to_string(),
            runtime: RuntimeType::Docker,
            status: Self::parse_status(status_str),
            created,
            image,
        })
    }

    fn start(&self, name: &str) -> Result<String, String> {
        let output = std::process::Command::new("docker")
            .args(["start", name])
            .output()
            .map_err(|e| format!("Failed to run docker: {}", e))?;

        if output.status.success() {
            Ok(format!("Container '{}' started", name))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("Failed to start container: {}", stderr))
        }
    }

    fn stop(&self, name: &str) -> Result<String, String> {
        let output = std::process::Command::new("docker")
            .args(["stop", name])
            .output()
            .map_err(|e| format!("Failed to run docker: {}", e))?;

        if output.status.success() {
            Ok(format!("Container '{}' stopped", name))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("Failed to stop container: {}", stderr))
        }
    }

    fn restart(&self, name: &str) -> Result<String, String> {
        let output = std::process::Command::new("docker")
            .args(["restart", name])
            .output()
            .map_err(|e| format!("Failed to run docker: {}", e))?;

        if output.status.success() {
            Ok(format!("Container '{}' restarted", name))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("Failed to restart container: {}", stderr))
        }
    }

    fn logs(&self, name: &str, follow: bool, tail: Option<u32>) -> Result<(), String> {
        let tail_str = tail.unwrap_or(50).to_string();
        let mut cmd = std::process::Command::new("docker");
        cmd.args(["logs", "--tail", &tail_str]);

        if follow {
            cmd.arg("-f");
            println!("Following logs for '{}' (Ctrl+C to stop)...\n", name);
        }

        cmd.arg(name);
        let status = cmd
            .status()
            .map_err(|e| format!("Failed to run docker: {}", e))?;

        if status.success() {
            Ok(())
        } else {
            Err("Failed to get logs".to_string())
        }
    }

    fn remove(&self, name: &str, force: bool) -> Result<String, String> {
        let mut cmd = std::process::Command::new("docker");
        cmd.arg("rm");

        if force {
            cmd.arg("-f");
        }

        cmd.arg(name);

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to run docker: {}", e))?;

        if output.status.success() {
            Ok(format!("Container '{}' removed", name))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("is running") {
                Err(format!(
                    "Container '{}' is running. Use --force or stop it first.",
                    name
                ))
            } else {
                Err(format!("Failed to remove container: {}", stderr))
            }
        }
    }

    fn is_available(&self) -> bool {
        std::process::Command::new("docker")
            .arg("version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn runtime_type(&self) -> RuntimeType {
        RuntimeType::Docker
    }
}

// === Machine Runtime (systemd/launchd) ===

pub struct MachineRuntime;

impl MachineRuntime {
    pub fn new() -> Self {
        MachineRuntime
    }

    fn detect_os() -> &'static str {
        #[cfg(target_os = "linux")]
        return "linux";

        #[cfg(target_os = "macos")]
        return "macos";

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        return "unknown";
    }

    fn get_service_status_linux() -> Result<CocoonStatus, String> {
        let output = std::process::Command::new("systemctl")
            .args(["--user", "is-active", "cocoon"])
            .output()
            .map_err(|e| format!("Failed to run systemctl: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(match stdout.as_str() {
            "active" => CocoonStatus::Running,
            "inactive" => CocoonStatus::Stopped,
            "activating" | "reloading" => CocoonStatus::Restarting,
            _ => CocoonStatus::Unknown(stdout),
        })
    }

    fn get_service_status_macos() -> Result<CocoonStatus, String> {
        let output = std::process::Command::new("launchctl")
            .args(["list"])
            .output()
            .map_err(|e| format!("Failed to run launchctl: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let is_running = stdout.lines().any(|line| line.contains("com.adi.cocoon"));

        Ok(if is_running {
            CocoonStatus::Running
        } else {
            CocoonStatus::Stopped
        })
    }

    fn is_service_installed() -> bool {
        let os = Self::detect_os();
        match os {
            "linux" => {
                let home = std::env::var("HOME").unwrap_or_default();
                std::path::Path::new(&format!("{}/.config/systemd/user/cocoon.service", home))
                    .exists()
            }
            "macos" => {
                let home = std::env::var("HOME").unwrap_or_default();
                std::path::Path::new(&format!(
                    "{}/Library/LaunchAgents/com.adi.cocoon.plist",
                    home
                ))
                .exists()
            }
            _ => false,
        }
    }
}

impl Runtime for MachineRuntime {
    fn list(&self) -> Result<Vec<CocoonInfo>, String> {
        if !Self::is_service_installed() {
            return Ok(vec![]);
        }

        let status = self.status("cocoon")?;
        Ok(vec![status])
    }

    fn status(&self, _name: &str) -> Result<CocoonInfo, String> {
        if !Self::is_service_installed() {
            return Err(
                "Machine service not installed. Install with: adi cocoon create --runtime machine"
                    .to_string(),
            );
        }

        let os = Self::detect_os();
        let status = match os {
            "linux" => Self::get_service_status_linux()?,
            "macos" => Self::get_service_status_macos()?,
            _ => return Err("Unsupported OS".to_string()),
        };

        Ok(CocoonInfo {
            name: "cocoon".to_string(),
            runtime: RuntimeType::Machine,
            status,
            created: None,
            image: None,
        })
    }

    fn start(&self, _name: &str) -> Result<String, String> {
        let os = Self::detect_os();

        match os {
            "linux" => {
                let output = std::process::Command::new("systemctl")
                    .args(["--user", "start", "cocoon"])
                    .output()
                    .map_err(|e| format!("Failed to start service: {}", e))?;

                if output.status.success() {
                    Ok("Service started".to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(format!("Failed to start service: {}", stderr))
                }
            }
            "macos" => {
                let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
                let plist = format!("{}/Library/LaunchAgents/com.adi.cocoon.plist", home);

                let output = std::process::Command::new("launchctl")
                    .args(["load", &plist])
                    .output()
                    .map_err(|e| format!("Failed to load service: {}", e))?;

                if output.status.success() {
                    Ok("Service loaded".to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(format!("Failed to load service: {}", stderr))
                }
            }
            _ => Err("Unsupported OS".to_string()),
        }
    }

    fn stop(&self, _name: &str) -> Result<String, String> {
        let os = Self::detect_os();

        match os {
            "linux" => {
                let output = std::process::Command::new("systemctl")
                    .args(["--user", "stop", "cocoon"])
                    .output()
                    .map_err(|e| format!("Failed to stop service: {}", e))?;

                if output.status.success() {
                    Ok("Service stopped".to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(format!("Failed to stop service: {}", stderr))
                }
            }
            "macos" => {
                let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
                let plist = format!("{}/Library/LaunchAgents/com.adi.cocoon.plist", home);

                let output = std::process::Command::new("launchctl")
                    .args(["unload", &plist])
                    .output()
                    .map_err(|e| format!("Failed to unload service: {}", e))?;

                if output.status.success() {
                    Ok("Service unloaded".to_string())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(format!("Failed to unload service: {}", stderr))
                }
            }
            _ => Err("Unsupported OS".to_string()),
        }
    }

    fn restart(&self, name: &str) -> Result<String, String> {
        self.stop(name)?;
        std::thread::sleep(std::time::Duration::from_secs(1));
        self.start(name)
    }

    fn logs(&self, _name: &str, follow: bool, tail: Option<u32>) -> Result<(), String> {
        let os = Self::detect_os();

        match os {
            "linux" => {
                let mut cmd = std::process::Command::new("journalctl");
                cmd.args(["--user", "-u", "cocoon"]);

                if follow {
                    cmd.arg("-f");
                }

                if let Some(n) = tail {
                    cmd.args(["-n", &n.to_string()]);
                }

                println!("Following logs (Ctrl+C to stop)...");
                cmd.status()
                    .map_err(|e| format!("Failed to view logs: {}", e))?;
                Ok(())
            }
            "macos" => {
                let mut cmd = std::process::Command::new("tail");

                if follow {
                    cmd.arg("-f");
                }

                if let Some(n) = tail {
                    cmd.arg("-n").arg(n.to_string());
                }

                cmd.arg("/tmp/cocoon.log");
                println!("Following logs (Ctrl+C to stop)...");
                cmd.status()
                    .map_err(|e| format!("Failed to view logs: {}", e))?;
                Ok(())
            }
            _ => Err("Unsupported OS".to_string()),
        }
    }

    fn remove(&self, _name: &str, _force: bool) -> Result<String, String> {
        // Stop service first
        let _ = self.stop("cocoon");

        let os = Self::detect_os();

        match os {
            "linux" => {
                let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
                let service_file = format!("{}/.config/systemd/user/cocoon.service", home);

                std::process::Command::new("systemctl")
                    .args(["--user", "disable", "cocoon"])
                    .status()
                    .ok();

                if std::path::Path::new(&service_file).exists() {
                    std::fs::remove_file(&service_file)
                        .map_err(|e| format!("Failed to remove service file: {}", e))?;
                }

                std::process::Command::new("systemctl")
                    .args(["--user", "daemon-reload"])
                    .status()
                    .ok();

                Ok("Service uninstalled".to_string())
            }
            "macos" => {
                let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
                let plist = format!("{}/Library/LaunchAgents/com.adi.cocoon.plist", home);

                if std::path::Path::new(&plist).exists() {
                    std::fs::remove_file(&plist)
                        .map_err(|e| format!("Failed to remove plist: {}", e))?;
                }

                Ok("Service uninstalled".to_string())
            }
            _ => Err("Unsupported OS".to_string()),
        }
    }

    fn is_available(&self) -> bool {
        let os = Self::detect_os();
        match os {
            "linux" => std::process::Command::new("systemctl")
                .arg("--user")
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false),
            "macos" => true, // launchd is always available on macOS
            _ => false,
        }
    }

    fn runtime_type(&self) -> RuntimeType {
        RuntimeType::Machine
    }
}

// === Unified Runtime Manager ===

pub struct RuntimeManager {
    docker: DockerRuntime,
    machine: MachineRuntime,
}

impl RuntimeManager {
    pub fn new() -> Self {
        RuntimeManager {
            docker: DockerRuntime::new(),
            machine: MachineRuntime::new(),
        }
    }

    /// List all cocoons across all runtimes
    pub fn list_all(&self) -> Result<Vec<CocoonInfo>, String> {
        let mut all = Vec::new();

        if self.docker.is_available() {
            if let Ok(docker_cocoons) = self.docker.list() {
                all.extend(docker_cocoons);
            }
        }

        if self.machine.is_available() {
            if let Ok(machine_cocoons) = self.machine.list() {
                all.extend(machine_cocoons);
            }
        }

        Ok(all)
    }

    /// Get a runtime by type
    pub fn get_runtime(&self, runtime_type: RuntimeType) -> &dyn Runtime {
        match runtime_type {
            RuntimeType::Docker => &self.docker,
            RuntimeType::Machine => &self.machine,
        }
    }

    /// Find a cocoon by name and return its runtime
    pub fn find_cocoon(&self, name: &str) -> Option<(CocoonInfo, RuntimeType)> {
        // Check Docker first
        if self.docker.is_available() {
            if let Ok(info) = self.docker.status(name) {
                return Some((info, RuntimeType::Docker));
            }
        }

        // Check Machine (only has one cocoon named "cocoon")
        if self.machine.is_available() && name == "cocoon" {
            if let Ok(info) = self.machine.status(name) {
                return Some((info, RuntimeType::Machine));
            }
        }

        None
    }

    /// Get available runtimes
    pub fn available_runtimes(&self) -> Vec<RuntimeType> {
        let mut runtimes = Vec::new();
        if self.docker.is_available() {
            runtimes.push(RuntimeType::Docker);
        }
        if self.machine.is_available() {
            runtimes.push(RuntimeType::Machine);
        }
        runtimes
    }
}
