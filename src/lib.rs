//! Cocoon Plugin
//!
//! Remote containerized worker with PTY support and signaling server connectivity.

pub mod adi_router;
mod core;
pub mod filesystem;
mod interactive;
mod runtime;
mod self_update;
pub mod services;
pub mod silk;
pub mod webrtc;

pub use adi_router::{AdiHandleResult, AdiRouter, AdiService, AdiServiceError, create_stream_channel, StreamSender};
pub use core::run;
pub use runtime::{CocoonInfo, CocoonStatus, Runtime, RuntimeManager, RuntimeType};
pub use silk::{AnsiToHtml, SilkSession};
pub use webrtc::WebRtcManager;

#[cfg(feature = "adi-tasks-core")]
pub use services::TasksService;

use base64::Engine;
use lib_plugin_abi_v3::{
    async_trait,
    cli::{CliCommand, CliCommands, CliContext, CliResult},
    Plugin, PluginContext, PluginMetadata, PluginType, Result as PluginResult, SERVICE_CLI_COMMANDS,
};

// === Service Management Helpers ===

fn detect_os() -> &'static str {
    #[cfg(target_os = "linux")]
    return "linux";

    #[cfg(target_os = "macos")]
    return "macos";

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return "unknown";
}

fn get_binary_path() -> Result<std::path::PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("Failed to get current binary path: {}", e))
}

fn generate_secret() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..36).map(|_| rng.gen()).collect();
    base64::engine::general_purpose::STANDARD.encode(&bytes)
}

pub fn service_install() -> Result<String, String> {
    let os = detect_os();

    match os {
        "linux" => install_systemd_service(),
        "macos" => install_launchd_service(),
        _ => Err("Unsupported OS for service installation".to_string()),
    }
}

fn install_systemd_service() -> Result<String, String> {
    let signaling_url = std::env::var("SIGNALING_SERVER_URL")
        .unwrap_or_else(|_| "ws://localhost:8080/ws".to_string());

    let home_dir =
        std::env::var("HOME").map_err(|_| "HOME environment variable not set".to_string())?;

    let service_dir = format!("{}/.config/systemd/user", home_dir);
    let service_file = format!("{}/cocoon.service", service_dir);

    std::fs::create_dir_all(&service_dir)
        .map_err(|e| format!("Failed to create service directory: {}", e))?;

    let config_dir = format!("{}/.config/cocoon", home_dir);
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("Failed to create config directory: {}", e))?;

    let secret_file = format!("{}/secret", config_dir);
    let secret = if std::path::Path::new(&secret_file).exists() {
        std::fs::read_to_string(&secret_file)
            .map_err(|e| format!("Failed to read secret: {}", e))?
            .trim()
            .to_string()
    } else {
        let new_secret = generate_secret();
        std::fs::write(&secret_file, &new_secret)
            .map_err(|e| format!("Failed to save secret: {}", e))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&secret_file, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("Failed to set secret permissions: {}", e))?;
        }

        new_secret
    };

    let setup_token = std::env::var("COCOON_SETUP_TOKEN").ok();
    let binary_path = get_binary_path()?;

    let mut service_content = format!(
        r#"[Unit]
Description=Cocoon - Remote containerized worker
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={} cocoon run
Restart=always
RestartSec=5
Environment=SIGNALING_SERVER_URL={}
Environment=COCOON_SECRET={}
"#,
        binary_path.display(),
        signaling_url,
        secret
    );

    if let Some(token) = setup_token {
        service_content.push_str(&format!("Environment=COCOON_SETUP_TOKEN={}\n", token));
    }

    service_content.push_str(
        r#"
[Install]
WantedBy=default.target
"#,
    );

    std::fs::write(&service_file, service_content)
        .map_err(|e| format!("Failed to write service file: {}", e))?;

    std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .map_err(|e| format!("Failed to reload systemd: {}", e))?;

    std::process::Command::new("systemctl")
        .args(["--user", "enable", "cocoon"])
        .status()
        .map_err(|e| format!("Failed to enable service: {}", e))?;

    Ok(format!(
        "Systemd service installed\n\nService file: {}\nSecret file: {}\n\nStart: adi cocoon start cocoon\nStatus: adi cocoon status cocoon\nLogs: adi cocoon logs cocoon",
        service_file, secret_file
    ))
}

fn install_launchd_service() -> Result<String, String> {
    let signaling_url = std::env::var("SIGNALING_SERVER_URL")
        .unwrap_or_else(|_| "ws://localhost:8080/ws".to_string());

    let home_dir =
        std::env::var("HOME").map_err(|_| "HOME environment variable not set".to_string())?;

    let plist_dir = format!("{}/Library/LaunchAgents", home_dir);
    let plist_file = format!("{}/com.adi.cocoon.plist", plist_dir);

    std::fs::create_dir_all(&plist_dir)
        .map_err(|e| format!("Failed to create LaunchAgents directory: {}", e))?;

    let config_dir = format!("{}/.config/cocoon", home_dir);
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("Failed to create config directory: {}", e))?;

    let secret_file = format!("{}/secret", config_dir);
    let secret = if std::path::Path::new(&secret_file).exists() {
        std::fs::read_to_string(&secret_file)
            .map_err(|e| format!("Failed to read secret: {}", e))?
            .trim()
            .to_string()
    } else {
        let new_secret = generate_secret();
        std::fs::write(&secret_file, &new_secret)
            .map_err(|e| format!("Failed to save secret: {}", e))?;
        new_secret
    };

    let setup_token = std::env::var("COCOON_SETUP_TOKEN").ok();
    let binary_path = get_binary_path()?;

    let mut plist_content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.adi.cocoon</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>cocoon</string>
        <string>run</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>SIGNALING_SERVER_URL</key>
        <string>{}</string>
        <key>COCOON_SECRET</key>
        <string>{}</string>
"#,
        binary_path.display(),
        signaling_url,
        secret
    );

    if let Some(token) = setup_token {
        plist_content.push_str(&format!(
            r#"        <key>COCOON_SETUP_TOKEN</key>
        <string>{}</string>
"#,
            token
        ));
    }

    plist_content.push_str(
        r#"    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/cocoon.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/cocoon.error.log</string>
</dict>
</plist>
"#,
    );

    std::fs::write(&plist_file, plist_content)
        .map_err(|e| format!("Failed to write plist file: {}", e))?;

    Ok(format!(
        "LaunchAgent plist created\n\nPlist file: {}\nSecret file: {}\n\nStart: adi cocoon start cocoon\nStatus: adi cocoon status cocoon\nLogs: adi cocoon logs cocoon",
        plist_file, secret_file
    ))
}

pub fn service_start() -> Result<String, String> {
    let manager = RuntimeManager::new();
    let runtime = manager.get_runtime(RuntimeType::Machine);
    runtime.start("cocoon")
}

/// Generate a unique container name
fn generate_container_name() -> String {
    let output = std::process::Command::new("docker")
        .args(["ps", "-a", "--format", "{{.Names}}"])
        .output();

    if let Ok(output) = output {
        let names = String::from_utf8_lossy(&output.stdout);
        let existing: Vec<&str> = names.lines().filter(|n| n.starts_with("cocoon-")).collect();

        if existing.is_empty() {
            return "cocoon-worker".to_string();
        }

        if !existing.contains(&"cocoon-worker") {
            return "cocoon-worker".to_string();
        }

        let mut num = 2;
        loop {
            let candidate = format!("cocoon-worker-{}", num);
            if !existing.contains(&candidate.as_str()) {
                return candidate;
            }
            num += 1;
        }
    }

    "cocoon-worker".to_string()
}

/// Create a Docker cocoon
fn create_docker_cocoon(
    name: &str,
    signaling_url: &str,
    setup_token: Option<&str>,
    cocoon_secret: Option<&str>,
) -> Result<String, String> {
    let mut docker_cmd = std::process::Command::new("docker");
    docker_cmd
        .arg("run")
        .arg("-d")
        .arg("--restart")
        .arg("unless-stopped")
        .arg("--name")
        .arg(name);

    // Add host mapping for .local domains
    if let Ok(url) = url::Url::parse(signaling_url) {
        if let Some(host) = url.host_str() {
            if host.ends_with(".local") {
                docker_cmd
                    .arg("--add-host")
                    .arg(format!("{}:host-gateway", host));
            }
        }
    }

    docker_cmd
        .arg("-e")
        .arg(format!("SIGNALING_SERVER_URL={}", signaling_url))
        .arg("-v")
        .arg(format!("{}:/cocoon", name));

    if let Some(secret) = cocoon_secret {
        docker_cmd
            .arg("-e")
            .arg(format!("COCOON_SECRET={}", secret));
    }

    if let Some(token) = setup_token {
        docker_cmd
            .arg("-e")
            .arg(format!("COCOON_SETUP_TOKEN={}", token));
    }

    docker_cmd.arg("docker-registry.the-ihor.com/cocoon:latest");

    println!("Creating Docker cocoon '{}'...", name);

    match docker_cmd.output() {
        Ok(output) if output.status.success() => {
            let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            println!("Container created: {}", container_id);
            println!("\nManage cocoon:");
            println!("  adi cocoon status {}", name);
            println!("  adi cocoon logs {} -f", name);
            println!("  adi cocoon stop {}", name);
            Ok(format!("Container '{}' created: {}", name, container_id))
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("Docker failed: {}", stderr))
        }
        Err(e) => Err(format!(
            "Failed to start Docker: {}. Make sure Docker is installed and running.",
            e
        )),
    }
}

fn get_help_text() -> &'static str {
    r#"Cocoon - Remote containerized worker

USAGE:
    adi cocoon [COMMAND] [ARGS]

COMMANDS:
    (no args)           Interactive mode - select actions from menu
    list, ls            List all cocoons (Docker and Machine)
    status <name>       Show cocoon status
    start <name>        Start a stopped cocoon
    stop <name>         Stop a running cocoon
    restart <name>      Restart a cocoon
    logs <name> [-f]    View cocoon logs (-f to follow)
    rm <name> [--force] Remove a cocoon
    create              Create a new cocoon (interactive)
    run                 Run cocoon natively in foreground
    check-update [name] Check for available updates
    update [name]       Update cocoon to latest version
    version             Show current version
    help                Show this help message

CREATE OPTIONS:
    --runtime TYPE      Runtime: docker or machine
    --name NAME         Container name (docker only)
    --url URL           Signaling server URL
    --token TOKEN       Setup token for auto-claim
    --secret SECRET     Pre-generated secret
    --start             Start service after create (machine only)

UPDATE OPTIONS:
    --all, -a           Update all cocoons

RUNTIMES:
    docker      Docker containers (prefix: cocoon-*)
                Update: Pulls latest image and recreates container
    machine     Native systemd/launchd service
                Update: Downloads latest binary and restarts service

EXAMPLES:
    # Interactive mode (recommended)
    adi cocoon

    # List all cocoons
    adi cocoon list

    # Control a specific cocoon
    adi cocoon start cocoon-worker
    adi cocoon stop cocoon-worker
    adi cocoon logs cocoon-worker -f

    # Create a Docker cocoon
    adi cocoon create --runtime docker --name my-worker --url wss://example.com/ws

    # Create a Machine (native service) cocoon
    adi cocoon create --runtime machine --url wss://example.com/ws --start

    # Check for updates (specific cocoon)
    adi cocoon check-update cocoon-worker

    # Check for updates (all cocoons)
    adi cocoon check-update

    # Update a specific cocoon
    adi cocoon update cocoon-worker

    # Update all cocoons
    adi cocoon update --all

ENVIRONMENT VARIABLES:
    SIGNALING_SERVER_URL    WebSocket URL (default: ws://localhost:8080/ws)
    COCOON_SECRET           Pre-generated secret for persistent device ID
    COCOON_SETUP_TOKEN      Setup token for auto-claim
"#
}

/// Cocoon Plugin
pub struct CocoonPlugin;

impl CocoonPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CocoonPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for CocoonPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: "adi.cocoon".to_string(),
            name: "Cocoon".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            plugin_type: PluginType::Core,
            author: Some("ADI Team".to_string()),
            description: Some("Remote containerized worker with PTY support".to_string()),
            category: None,
        }
    }

    async fn init(&mut self, _ctx: &PluginContext) -> PluginResult<()> {
        Ok(())
    }

    async fn shutdown(&self) -> PluginResult<()> {
        Ok(())
    }

    fn provides(&self) -> Vec<&'static str> {
        vec![SERVICE_CLI_COMMANDS]
    }
}

#[async_trait]
impl CliCommands for CocoonPlugin {
    async fn list_commands(&self) -> Vec<CliCommand> {
        vec![
            CliCommand {
                name: "".to_string(),
                description: "Interactive mode (default)".to_string(),
                usage: "".to_string(),
                has_subcommands: false,
            },
            CliCommand {
                name: "list".to_string(),
                description: "List all cocoons".to_string(),
                usage: "list".to_string(),
                has_subcommands: false,
            },
            CliCommand {
                name: "status".to_string(),
                description: "Show cocoon status".to_string(),
                usage: "status <name>".to_string(),
                has_subcommands: false,
            },
            CliCommand {
                name: "start".to_string(),
                description: "Start a cocoon".to_string(),
                usage: "start <name>".to_string(),
                has_subcommands: false,
            },
            CliCommand {
                name: "stop".to_string(),
                description: "Stop a cocoon".to_string(),
                usage: "stop <name>".to_string(),
                has_subcommands: false,
            },
            CliCommand {
                name: "restart".to_string(),
                description: "Restart a cocoon".to_string(),
                usage: "restart <name>".to_string(),
                has_subcommands: false,
            },
            CliCommand {
                name: "logs".to_string(),
                description: "View cocoon logs".to_string(),
                usage: "logs <name> [-f]".to_string(),
                has_subcommands: false,
            },
            CliCommand {
                name: "rm".to_string(),
                description: "Remove a cocoon".to_string(),
                usage: "rm <name> [--force]".to_string(),
                has_subcommands: false,
            },
            CliCommand {
                name: "create".to_string(),
                description: "Create a new cocoon".to_string(),
                usage: "create [--runtime docker|machine] [--name NAME] [--url URL]".to_string(),
                has_subcommands: false,
            },
            CliCommand {
                name: "run".to_string(),
                description: "Run cocoon natively in foreground".to_string(),
                usage: "run".to_string(),
                has_subcommands: false,
            },
            CliCommand {
                name: "check-update".to_string(),
                description: "Check for available updates".to_string(),
                usage: "check-update [name]".to_string(),
                has_subcommands: false,
            },
            CliCommand {
                name: "update".to_string(),
                description: "Update cocoon to latest version".to_string(),
                usage: "update [name] [--all]".to_string(),
                has_subcommands: false,
            },
            CliCommand {
                name: "version".to_string(),
                description: "Show current version".to_string(),
                usage: "version".to_string(),
                has_subcommands: false,
            },
        ]
    }

    async fn run_command(&self, ctx: &CliContext) -> PluginResult<CliResult> {
        let subcommand = ctx.subcommand.as_deref().unwrap_or("");
        let args: Vec<String> = ctx.args.clone();
        let manager = RuntimeManager::new();

        let result = match subcommand {
            // Interactive mode (no args or explicit "interactive")
            "" | "interactive" | "i" => {
                if let Err(e) = interactive::run_interactive(&manager) {
                    return Ok(CliResult::error(e));
                }
                Ok("Interactive mode exited".to_string())
            }

            // List all cocoons
            "list" | "ls" | "ps" => {
                if let Err(e) = interactive::handle_list(&manager) {
                    return Ok(CliResult::error(e));
                }
                Ok("Listed cocoons".to_string())
            }

            // Status of a specific cocoon
            "status" => {
                let name = args.first().map(|s| s.as_str());

                if let Some(name) = name {
                    match manager.find_cocoon(name) {
                        Some((_, runtime_type)) => {
                            let runtime = manager.get_runtime(runtime_type);
                            match runtime.status(name) {
                                Ok(info) => {
                                    let reset = "\x1b[0m";
                                    println!("\nCocoon: {}", info.name);
                                    println!("Runtime: {}", info.runtime);
                                    println!(
                                        "Status: {}{}{}{}",
                                        info.status_color(),
                                        info.status_icon(),
                                        info.status,
                                        reset
                                    );
                                    if let Some(image) = &info.image {
                                        println!("Image: {}", image);
                                    }
                                    if let Some(created) = &info.created {
                                        println!("Created: {}", created);
                                    }
                                    println!();
                                    Ok(format!("Status: {}", info.status))
                                }
                                Err(e) => Err(e),
                            }
                        }
                        None => Err(format!("Cocoon '{}' not found", name)),
                    }
                } else {
                    // Interactive selection
                    if let Err(e) = interactive::run_interactive(&manager) {
                        return Ok(CliResult::error(e));
                    }
                    Ok("Done".to_string())
                }
            }

            // Start a cocoon
            "start" => {
                let name = args.first().map(|s| s.as_str());

                if let Some(name) = name {
                    match manager.find_cocoon(name) {
                        Some((_, runtime_type)) => {
                            let runtime = manager.get_runtime(runtime_type);
                            println!("Starting '{}'...", name);
                            runtime.start(name)
                        }
                        None => Err(format!(
                            "Cocoon '{}' not found. Use 'adi cocoon list' to see available cocoons.",
                            name
                        )),
                    }
                } else {
                    // Interactive selection
                    if let Err(e) = interactive::run_interactive(&manager) {
                        return Ok(CliResult::error(e));
                    }
                    Ok("Done".to_string())
                }
            }

            // Stop a cocoon
            "stop" => {
                let name = args.first().map(|s| s.as_str());

                if let Some(name) = name {
                    match manager.find_cocoon(name) {
                        Some((_, runtime_type)) => {
                            let runtime = manager.get_runtime(runtime_type);
                            println!("Stopping '{}'...", name);
                            runtime.stop(name)
                        }
                        None => Err(format!("Cocoon '{}' not found", name)),
                    }
                } else {
                    if let Err(e) = interactive::run_interactive(&manager) {
                        return Ok(CliResult::error(e));
                    }
                    Ok("Done".to_string())
                }
            }

            // Restart a cocoon
            "restart" => {
                let name = args.first().map(|s| s.as_str());

                if let Some(name) = name {
                    match manager.find_cocoon(name) {
                        Some((_, runtime_type)) => {
                            let runtime = manager.get_runtime(runtime_type);
                            println!("Restarting '{}'...", name);
                            runtime.restart(name)
                        }
                        None => Err(format!("Cocoon '{}' not found", name)),
                    }
                } else {
                    if let Err(e) = interactive::run_interactive(&manager) {
                        return Ok(CliResult::error(e));
                    }
                    Ok("Done".to_string())
                }
            }

            // View logs
            "logs" => {
                let name = args.first().map(|s| s.as_str());
                let follow = args.iter().any(|a| a == "-f" || a == "--follow");
                let tail = args
                    .iter()
                    .position(|arg| arg == "--tail")
                    .and_then(|idx| args.get(idx + 1))
                    .and_then(|s| s.parse().ok());

                if let Some(name) = name {
                    match manager.find_cocoon(name) {
                        Some((_, runtime_type)) => {
                            let runtime = manager.get_runtime(runtime_type);
                            match runtime.logs(name, follow, tail) {
                                Ok(()) => Ok("Logs displayed".to_string()),
                                Err(e) => Err(e),
                            }
                        }
                        None => Err(format!("Cocoon '{}' not found", name)),
                    }
                } else {
                    if let Err(e) = interactive::run_interactive(&manager) {
                        return Ok(CliResult::error(e));
                    }
                    Ok("Done".to_string())
                }
            }

            // Remove a cocoon
            "rm" | "remove" => {
                let name = args.first().map(|s| s.as_str());
                let force = args.iter().any(|a| a == "-f" || a == "--force");

                if let Some(name) = name {
                    match manager.find_cocoon(name) {
                        Some((_, runtime_type)) => {
                            let runtime = manager.get_runtime(runtime_type);
                            println!("Removing '{}'...", name);
                            runtime.remove(name, force)
                        }
                        None => Err(format!("Cocoon '{}' not found", name)),
                    }
                } else {
                    if let Err(e) = interactive::run_interactive(&manager) {
                        return Ok(CliResult::error(e));
                    }
                    Ok("Done".to_string())
                }
            }

            // Create a new cocoon
            "create" | "new" => {
                let runtime_arg = args
                    .iter()
                    .position(|arg| arg == "--runtime" || arg == "-r")
                    .and_then(|idx| args.get(idx + 1))
                    .map(|s| s.as_str());

                if let Some(runtime_str) = runtime_arg {
                    // Non-interactive create with runtime specified
                    let runtime_type = RuntimeType::from_str(runtime_str).ok_or_else(|| {
                        format!(
                            "Invalid runtime '{}'. Use 'docker' or 'machine'.",
                            runtime_str
                        )
                    });

                    match runtime_type {
                        Ok(RuntimeType::Docker) => {
                            let name = args
                                .iter()
                                .position(|arg| arg == "--name")
                                .and_then(|idx| args.get(idx + 1))
                                .map(|s| s.to_string())
                                .unwrap_or_else(generate_container_name);

                            let signaling_url = args
                                .iter()
                                .position(|arg| arg == "--url")
                                .and_then(|idx| args.get(idx + 1))
                                .map(|s| s.to_string())
                                .or_else(|| std::env::var("SIGNALING_SERVER_URL").ok())
                                .unwrap_or_else(|| "ws://localhost:8080/ws".to_string());

                            let setup_token = args
                                .iter()
                                .position(|arg| arg == "--token")
                                .and_then(|idx| args.get(idx + 1))
                                .map(|s| s.to_string())
                                .or_else(|| std::env::var("COCOON_SETUP_TOKEN").ok());

                            let cocoon_secret = args
                                .iter()
                                .position(|arg| arg == "--secret")
                                .and_then(|idx| args.get(idx + 1))
                                .map(|s| s.to_string())
                                .or_else(|| std::env::var("COCOON_SECRET").ok());

                            create_docker_cocoon(
                                &name,
                                &signaling_url,
                                setup_token.as_deref(),
                                cocoon_secret.as_deref(),
                            )
                        }
                        Ok(RuntimeType::Machine) => {
                            // Stop existing service first (ignore errors - may not be running)
                            let runtime = manager.get_runtime(RuntimeType::Machine);
                            let _ = runtime.stop("cocoon");

                            match service_install() {
                                Ok(msg) => {
                                    println!("{}", msg);
                                    if args.iter().any(|a| a == "--start") {
                                        match service_start() {
                                            Ok(start_msg) => println!("{}", start_msg),
                                            Err(e) => {
                                                println!("Warning: Failed to start service: {}", e)
                                            }
                                        }
                                    }
                                    Ok("Machine cocoon created".to_string())
                                }
                                Err(e) => Err(e),
                            }
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    // Interactive create
                    if let Err(e) = interactive::run_interactive(&manager) {
                        return Ok(CliResult::error(e));
                    }
                    Ok("Done".to_string())
                }
            }

            // Run natively in foreground (blocking - for use by launchd/systemd)
            "run" => {
                let rt = match tokio::runtime::Runtime::new() {
                    Ok(rt) => rt,
                    Err(e) => {
                        return Ok(CliResult::error(format!("Failed to create runtime: {}", e)));
                    }
                };

                rt.block_on(async {
                    if let Err(e) = core::run().await {
                        eprintln!("Cocoon error: {}", e);
                    }
                });

                Ok("Cocoon stopped".to_string())
            }

            // Check for updates
            "check-update" | "check" => {
                let name = args.first().map(|s| s.as_str());

                if let Some(name) = name {
                    // Check specific cocoon
                    match manager.find_cocoon(name) {
                        Some((_, runtime_type)) => {
                            let runtime = manager.get_runtime(runtime_type);
                            match runtime.check_update(name) {
                                Ok(msg) => {
                                    println!("{}", msg);
                                    Ok(msg)
                                }
                                Err(e) => Err(e),
                            }
                        }
                        None => Err(format!(
                            "Cocoon '{}' not found. Use 'adi cocoon list' to see available cocoons.",
                            name
                        )),
                    }
                } else {
                    // Check all cocoons
                    match manager.list_all() {
                        Ok(cocoons) if cocoons.is_empty() => {
                            println!("No cocoons found. Create one with: adi cocoon create");
                            Ok("No cocoons found".to_string())
                        }
                        Ok(cocoons) => {
                            let mut results = Vec::new();
                            for info in cocoons {
                                let runtime = manager.get_runtime(info.runtime);
                                println!("--- {} ({}) ---", info.name, info.runtime);
                                match runtime.check_update(&info.name) {
                                    Ok(msg) => {
                                        println!("{}", msg);
                                        results.push(format!("{}: OK", info.name));
                                    }
                                    Err(e) => {
                                        println!("Error: {}\n", e);
                                        results.push(format!("{}: Error", info.name));
                                    }
                                }
                            }
                            Ok(results.join(", "))
                        }
                        Err(e) => Err(e),
                    }
                }
            }

            // Perform update on a cocoon
            "update" | "upgrade" | "self-update" => {
                let name = args.first().map(|s| s.as_str());

                if let Some(name) = name {
                    // Update specific cocoon
                    match manager.find_cocoon(name) {
                        Some((_, runtime_type)) => {
                            let runtime = manager.get_runtime(runtime_type);
                            match runtime.update(name) {
                                Ok(msg) => {
                                    println!("{}", msg);
                                    Ok(msg)
                                }
                                Err(e) => Err(e),
                            }
                        }
                        None => Err(format!(
                            "Cocoon '{}' not found. Use 'adi cocoon list' to see available cocoons.",
                            name
                        )),
                    }
                } else {
                    // Interactive selection or update all
                    let all_flag = args.iter().any(|a| a == "--all" || a == "-a");

                    if all_flag {
                        // Update all cocoons
                        match manager.list_all() {
                            Ok(cocoons) if cocoons.is_empty() => {
                                println!("No cocoons found. Create one with: adi cocoon create");
                                Ok("No cocoons found".to_string())
                            }
                            Ok(cocoons) => {
                                let mut results = Vec::new();
                                for info in cocoons {
                                    let runtime = manager.get_runtime(info.runtime);
                                    println!(
                                        "\n=== Updating {} ({}) ===\n",
                                        info.name, info.runtime
                                    );
                                    match runtime.update(&info.name) {
                                        Ok(msg) => {
                                            println!("{}", msg);
                                            results.push(format!("{}: Updated", info.name));
                                        }
                                        Err(e) => {
                                            println!("Error: {}", e);
                                            results.push(format!("{}: Failed", info.name));
                                        }
                                    }
                                }
                                println!("\n=== Update Summary ===");
                                for r in &results {
                                    println!("  {}", r);
                                }
                                Ok(results.join(", "))
                            }
                            Err(e) => Err(e),
                        }
                    } else {
                        // Interactive mode
                        if let Err(e) = interactive::run_interactive(&manager) {
                            return Ok(CliResult::error(e));
                        }
                        Ok("Done".to_string())
                    }
                }
            }

            // Show version
            "version" | "-v" | "-V" | "--version" => {
                let version = env!("CARGO_PKG_VERSION");
                println!("cocoon {}", version);
                Ok(format!("cocoon {}", version))
            }

            // Help
            "help" | "-h" | "--help" => Ok(get_help_text().to_string()),

            // Unknown command
            _ => {
                println!("Unknown command: {}", subcommand);
                println!("Run 'adi cocoon help' for usage information.");
                Err(format!("Unknown command: {}", subcommand))
            }
        };

        match result {
            Ok(output) => Ok(CliResult::success(output)),
            Err(e) => Ok(CliResult::error(e)),
        }
    }
}

/// Create the plugin instance (v3 entry point)
#[no_mangle]
pub fn plugin_create() -> Box<dyn Plugin> {
    Box::new(CocoonPlugin::new())
}

/// Create the CLI commands interface
#[no_mangle]
pub fn plugin_create_cli() -> Box<dyn CliCommands> {
    Box::new(CocoonPlugin::new())
}
