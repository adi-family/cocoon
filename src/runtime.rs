//! Runtime abstraction for different cocoon backends.
//!
//! Supports Docker containers and Machine (native systemd/launchd) services.

use crate::self_update;
use lib_console_output::{out_info, KeyValue, Renderable};
use std::fmt;

use lib_daemon_client::DaemonClient;

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

    /// Update a cocoon to the latest version
    fn update(&self, name: &str) -> Result<String, String>;

    /// Check for available updates
    fn check_update(&self, name: &str) -> Result<String, String>;
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
            out_info!("Following logs for '{}' (Ctrl+C to stop)...", name);
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

    fn update(&self, name: &str) -> Result<String, String> {
        out_info!("Updating Docker cocoon '{}'...", name);

        // Check if container exists
        let _ = self.status(name)?;

        // Pull latest image
        let updated = self_update::docker::pull_latest_image("latest")?;

        if !updated {
            return Ok("Already running the latest image.".to_string());
        }

        // Recreate container with new image
        let result = self_update::docker::recreate_container(name, "latest")?;

        Ok(format!(
            "Update complete!\n  {}\n\nThe cocoon is now running the latest image.",
            result
        ))
    }

    fn check_update(&self, name: &str) -> Result<String, String> {
        out_info!("Checking for updates for Docker cocoon '{}'...", name);

        // Check if container exists
        let info = self.status(name)?;

        let (needs_update, details) = self_update::docker::check_for_updates("latest")?;

        let mut kv = KeyValue::new()
            .entry("Cocoon", name)
            .entry("Runtime", "Docker")
            .entry("Status", info.status.to_string());
        if let Some(ref image) = info.image {
            kv = kv.entry("Image", image);
        }
        kv = kv.entry("Details", &details);
        kv.print();

        let hint = if needs_update {
            format!("Run 'adi cocoon update {}' to update.", name)
        } else {
            "Tip: Run 'adi cocoon update' to pull the latest image.".to_string()
        };
        out_info!("{}", hint);

        Ok(hint)
    }
}

// === Machine Runtime (via ADI daemon) ===

const SERVICE_NAME: &str = "adi.cocoon";

fn get_runtime() -> &'static tokio::runtime::Runtime {
    crate::get_runtime()
}

fn map_service_state(state: lib_daemon_client::ServiceState) -> CocoonStatus {
    match state {
        lib_daemon_client::ServiceState::Running => CocoonStatus::Running,
        lib_daemon_client::ServiceState::Stopped => CocoonStatus::Stopped,
        lib_daemon_client::ServiceState::Starting => CocoonStatus::Restarting,
        lib_daemon_client::ServiceState::Stopping => CocoonStatus::Restarting,
        lib_daemon_client::ServiceState::Failed => CocoonStatus::Unknown("failed".to_string()),
    }
}

fn find_cocoon_service(
    services: &[lib_daemon_client::ServiceInfo],
) -> Option<&lib_daemon_client::ServiceInfo> {
    services.iter().find(|s| s.name == SERVICE_NAME)
}

pub struct MachineRuntime;

impl MachineRuntime {
    pub fn new() -> Self {
        MachineRuntime
    }
}

impl Runtime for MachineRuntime {
    fn list(&self) -> Result<Vec<CocoonInfo>, String> {
        let client = DaemonClient::new();
        let services = get_runtime()
            .block_on(client.list_services())
            .map_err(|e| format!("Failed to list services: {}", e))?;

        let Some(svc) = find_cocoon_service(&services) else {
            return Ok(vec![]);
        };

        Ok(vec![CocoonInfo {
            name: "cocoon".to_string(),
            runtime: RuntimeType::Machine,
            status: map_service_state(svc.state),
            created: None,
            image: None,
        }])
    }

    fn status(&self, _name: &str) -> Result<CocoonInfo, String> {
        let client = DaemonClient::new();
        let services = get_runtime()
            .block_on(client.list_services())
            .map_err(|e| format!("Failed to list services: {}", e))?;

        let svc = find_cocoon_service(&services).ok_or_else(|| {
            "Cocoon service not registered. Start with: adi cocoon create --runtime machine"
                .to_string()
        })?;

        Ok(CocoonInfo {
            name: "cocoon".to_string(),
            runtime: RuntimeType::Machine,
            status: map_service_state(svc.state),
            created: None,
            image: None,
        })
    }

    fn start(&self, _name: &str) -> Result<String, String> {
        crate::ensure_daemon_running()?;
        Ok("Cocoon service started".to_string())
    }

    fn stop(&self, _name: &str) -> Result<String, String> {
        let client = DaemonClient::new();
        get_runtime()
            .block_on(client.stop_service(SERVICE_NAME, false))
            .map_err(|e| format!("Failed to stop cocoon service: {}", e))?;
        Ok("Cocoon service stopped".to_string())
    }

    fn restart(&self, _name: &str) -> Result<String, String> {
        let client = DaemonClient::new();
        get_runtime()
            .block_on(client.restart_service(SERVICE_NAME))
            .map_err(|e| format!("Failed to restart cocoon service: {}", e))?;
        Ok("Cocoon service restarted".to_string())
    }

    fn logs(&self, _name: &str, follow: bool, tail: Option<u32>) -> Result<(), String> {
        if follow {
            // DaemonClient.service_logs doesn't stream — use platform commands for follow
            #[cfg(target_os = "linux")]
            {
                let mut cmd = std::process::Command::new("journalctl");
                cmd.args(["--user", "-u", "adi-daemon", "-f"]);
                if let Some(n) = tail {
                    cmd.args(["-n", &n.to_string()]);
                }
                out_info!("Following logs (Ctrl+C to stop)...");
                cmd.status()
                    .map_err(|e| format!("Failed to view logs: {}", e))?;
                return Ok(());
            }

            #[cfg(target_os = "macos")]
            {
                let log_path = lib_daemon_client::paths::daemon_log_path();
                let mut cmd = std::process::Command::new("tail");
                cmd.arg("-f");
                if let Some(n) = tail {
                    cmd.arg("-n").arg(n.to_string());
                }
                cmd.arg(log_path);
                out_info!("Following logs (Ctrl+C to stop)...");
                cmd.status()
                    .map_err(|e| format!("Failed to view logs: {}", e))?;
                return Ok(());
            }

            #[allow(unreachable_code)]
            Err("Unsupported OS".to_string())
        } else {
            let client = DaemonClient::new();
            let lines = tail.unwrap_or(50) as usize;
            let log_lines = get_runtime()
                .block_on(client.service_logs(SERVICE_NAME, lines))
                .map_err(|e| format!("Failed to get logs: {}", e))?;
            for line in &log_lines {
                out_info!("{}", line);
            }
            Ok(())
        }
    }

    fn remove(&self, _name: &str, _force: bool) -> Result<String, String> {
        let client = DaemonClient::new();
        get_runtime()
            .block_on(client.stop_service(SERVICE_NAME, true))
            .map_err(|e| format!("Failed to stop cocoon service: {}", e))?;
        Ok("Cocoon service stopped".to_string())
    }

    fn is_available(&self) -> bool {
        true
    }

    fn runtime_type(&self) -> RuntimeType {
        RuntimeType::Machine
    }

    fn update(&self, _name: &str) -> Result<String, String> {
        out_info!("Updating Machine cocoon...");

        let client = DaemonClient::new();
        let services = get_runtime()
            .block_on(client.list_services())
            .unwrap_or_default();

        if find_cocoon_service(&services).is_none() {
            return Err(
                "Cocoon service not registered. Start with: adi cocoon create --runtime machine"
                    .to_string(),
            );
        }

        self_update::machine::update_and_restart()
    }

    fn check_update(&self, _name: &str) -> Result<String, String> {
        out_info!("Checking for updates for Machine cocoon...");

        let client = DaemonClient::new();
        let services = get_runtime()
            .block_on(client.list_services())
            .unwrap_or_default();

        if find_cocoon_service(&services).is_none() {
            return Err(
                "Cocoon service not registered. Start with: adi cocoon create --runtime machine"
                    .to_string(),
            );
        }

        let check_result = self_update::check_for_updates()?;
        Ok(self_update::format_check_result(&check_result))
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
