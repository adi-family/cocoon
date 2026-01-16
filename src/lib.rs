//! Cocoon Plugin
//!
//! Remote containerized worker with PTY support and signaling server connectivity.

mod core;
mod interactive;
mod runtime;
pub mod silk;

pub use core::run;
pub use runtime::{CocoonInfo, CocoonStatus, Runtime, RuntimeManager, RuntimeType};
pub use silk::{AnsiToHtml, SilkSession};

use abi_stable::std_types::{ROption, RResult, RStr, RString, RVec};
use base64::Engine;
use lib_plugin_abi::{
    PluginContext, PluginInfo, PluginVTable, ServiceDescriptor, ServiceError, ServiceHandle,
    ServiceMethod, ServiceVTable, ServiceVersion,
};
use std::ffi::c_void;

/// Plugin-specific CLI service ID
const SERVICE_CLI: &str = "adi.cocoon.cli";

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

// === CLI Service Implementation ===

extern "C" fn cli_invoke(
    _handle: *const c_void,
    method: RStr<'_>,
    args: RStr<'_>,
) -> RResult<RString, ServiceError> {
    match method.as_str() {
        "run_command" => {
            let context: serde_json::Value = match serde_json::from_str(args.as_str()) {
                Ok(v) => v,
                Err(e) => {
                    return RResult::RErr(ServiceError::new(1, format!("Invalid args: {}", e)))
                }
            };

            let cmd_args: Vec<String> = context
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let subcommand = cmd_args.first().map(|s| s.as_str()).unwrap_or("");
            let manager = RuntimeManager::new();

            match subcommand {
                // Interactive mode (no args or explicit "interactive")
                "" | "interactive" | "i" => {
                    if let Err(e) = interactive::run_interactive(&manager) {
                        return RResult::RErr(ServiceError::new(1, e));
                    }
                    RResult::ROk(RString::from("Interactive mode exited"))
                }

                // List all cocoons
                "list" | "ls" | "ps" => {
                    if let Err(e) = interactive::handle_list(&manager) {
                        return RResult::RErr(ServiceError::new(1, e));
                    }
                    RResult::ROk(RString::from("Listed cocoons"))
                }

                // Status of a specific cocoon
                "status" => {
                    let name = cmd_args.get(1).map(|s| s.as_str());

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
                                        RResult::ROk(RString::from(format!(
                                            "Status: {}",
                                            info.status
                                        )))
                                    }
                                    Err(e) => RResult::RErr(ServiceError::new(1, e)),
                                }
                            }
                            None => RResult::RErr(ServiceError::new(
                                1,
                                format!("Cocoon '{}' not found", name),
                            )),
                        }
                    } else {
                        // Interactive selection
                        if let Err(e) = interactive::run_interactive(&manager) {
                            return RResult::RErr(ServiceError::new(1, e));
                        }
                        RResult::ROk(RString::from("Done"))
                    }
                }

                // Start a cocoon
                "start" => {
                    let name = cmd_args.get(1).map(|s| s.as_str());

                    if let Some(name) = name {
                        match manager.find_cocoon(name) {
                            Some((_, runtime_type)) => {
                                let runtime = manager.get_runtime(runtime_type);
                                println!("Starting '{}'...", name);
                                match runtime.start(name) {
                                    Ok(msg) => {
                                        println!("{}", msg);
                                        RResult::ROk(RString::from(msg))
                                    }
                                    Err(e) => RResult::RErr(ServiceError::new(1, e)),
                                }
                            }
                            None => RResult::RErr(ServiceError::new(
                                1,
                                format!(
                                    "Cocoon '{}' not found. Use 'adi cocoon list' to see available cocoons.",
                                    name
                                ),
                            )),
                        }
                    } else {
                        // Interactive selection
                        if let Err(e) = interactive::run_interactive(&manager) {
                            return RResult::RErr(ServiceError::new(1, e));
                        }
                        RResult::ROk(RString::from("Done"))
                    }
                }

                // Stop a cocoon
                "stop" => {
                    let name = cmd_args.get(1).map(|s| s.as_str());

                    if let Some(name) = name {
                        match manager.find_cocoon(name) {
                            Some((_, runtime_type)) => {
                                let runtime = manager.get_runtime(runtime_type);
                                println!("Stopping '{}'...", name);
                                match runtime.stop(name) {
                                    Ok(msg) => {
                                        println!("{}", msg);
                                        RResult::ROk(RString::from(msg))
                                    }
                                    Err(e) => RResult::RErr(ServiceError::new(1, e)),
                                }
                            }
                            None => RResult::RErr(ServiceError::new(
                                1,
                                format!("Cocoon '{}' not found", name),
                            )),
                        }
                    } else {
                        if let Err(e) = interactive::run_interactive(&manager) {
                            return RResult::RErr(ServiceError::new(1, e));
                        }
                        RResult::ROk(RString::from("Done"))
                    }
                }

                // Restart a cocoon
                "restart" => {
                    let name = cmd_args.get(1).map(|s| s.as_str());

                    if let Some(name) = name {
                        match manager.find_cocoon(name) {
                            Some((_, runtime_type)) => {
                                let runtime = manager.get_runtime(runtime_type);
                                println!("Restarting '{}'...", name);
                                match runtime.restart(name) {
                                    Ok(msg) => {
                                        println!("{}", msg);
                                        RResult::ROk(RString::from(msg))
                                    }
                                    Err(e) => RResult::RErr(ServiceError::new(1, e)),
                                }
                            }
                            None => RResult::RErr(ServiceError::new(
                                1,
                                format!("Cocoon '{}' not found", name),
                            )),
                        }
                    } else {
                        if let Err(e) = interactive::run_interactive(&manager) {
                            return RResult::RErr(ServiceError::new(1, e));
                        }
                        RResult::ROk(RString::from("Done"))
                    }
                }

                // View logs
                "logs" => {
                    let name = cmd_args.get(1).map(|s| s.as_str());
                    let follow = cmd_args.iter().any(|a| a == "-f" || a == "--follow");
                    let tail = cmd_args
                        .iter()
                        .position(|arg| arg == "--tail")
                        .and_then(|idx| cmd_args.get(idx + 1))
                        .and_then(|s| s.parse().ok());

                    if let Some(name) = name {
                        match manager.find_cocoon(name) {
                            Some((_, runtime_type)) => {
                                let runtime = manager.get_runtime(runtime_type);
                                match runtime.logs(name, follow, tail) {
                                    Ok(()) => RResult::ROk(RString::from("Logs displayed")),
                                    Err(e) => RResult::RErr(ServiceError::new(1, e)),
                                }
                            }
                            None => RResult::RErr(ServiceError::new(
                                1,
                                format!("Cocoon '{}' not found", name),
                            )),
                        }
                    } else {
                        if let Err(e) = interactive::run_interactive(&manager) {
                            return RResult::RErr(ServiceError::new(1, e));
                        }
                        RResult::ROk(RString::from("Done"))
                    }
                }

                // Remove a cocoon
                "rm" | "remove" => {
                    let name = cmd_args.get(1).map(|s| s.as_str());
                    let force = cmd_args.iter().any(|a| a == "-f" || a == "--force");

                    if let Some(name) = name {
                        match manager.find_cocoon(name) {
                            Some((_, runtime_type)) => {
                                let runtime = manager.get_runtime(runtime_type);
                                println!("Removing '{}'...", name);
                                match runtime.remove(name, force) {
                                    Ok(msg) => {
                                        println!("{}", msg);
                                        RResult::ROk(RString::from(msg))
                                    }
                                    Err(e) => RResult::RErr(ServiceError::new(1, e)),
                                }
                            }
                            None => RResult::RErr(ServiceError::new(
                                1,
                                format!("Cocoon '{}' not found", name),
                            )),
                        }
                    } else {
                        if let Err(e) = interactive::run_interactive(&manager) {
                            return RResult::RErr(ServiceError::new(1, e));
                        }
                        RResult::ROk(RString::from("Done"))
                    }
                }

                // Create a new cocoon
                "create" | "new" => {
                    let runtime_arg = cmd_args
                        .iter()
                        .position(|arg| arg == "--runtime" || arg == "-r")
                        .and_then(|idx| cmd_args.get(idx + 1))
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
                                let name = cmd_args
                                    .iter()
                                    .position(|arg| arg == "--name")
                                    .and_then(|idx| cmd_args.get(idx + 1))
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(generate_container_name);

                                let signaling_url = cmd_args
                                    .iter()
                                    .position(|arg| arg == "--url")
                                    .and_then(|idx| cmd_args.get(idx + 1))
                                    .map(|s| s.to_string())
                                    .or_else(|| std::env::var("SIGNALING_SERVER_URL").ok())
                                    .unwrap_or_else(|| "ws://localhost:8080/ws".to_string());

                                let setup_token = cmd_args
                                    .iter()
                                    .position(|arg| arg == "--token")
                                    .and_then(|idx| cmd_args.get(idx + 1))
                                    .map(|s| s.to_string())
                                    .or_else(|| std::env::var("COCOON_SETUP_TOKEN").ok());

                                let cocoon_secret = cmd_args
                                    .iter()
                                    .position(|arg| arg == "--secret")
                                    .and_then(|idx| cmd_args.get(idx + 1))
                                    .map(|s| s.to_string())
                                    .or_else(|| std::env::var("COCOON_SECRET").ok());

                                match create_docker_cocoon(
                                    &name,
                                    &signaling_url,
                                    setup_token.as_deref(),
                                    cocoon_secret.as_deref(),
                                ) {
                                    Ok(msg) => RResult::ROk(RString::from(msg)),
                                    Err(e) => RResult::RErr(ServiceError::new(1, e)),
                                }
                            }
                            Ok(RuntimeType::Machine) => match service_install() {
                                Ok(msg) => {
                                    println!("{}", msg);
                                    if cmd_args.iter().any(|a| a == "--start") {
                                        match service_start() {
                                            Ok(start_msg) => println!("{}", start_msg),
                                            Err(e) => {
                                                println!("Warning: Failed to start service: {}", e)
                                            }
                                        }
                                    }
                                    RResult::ROk(RString::from("Machine cocoon created"))
                                }
                                Err(e) => RResult::RErr(ServiceError::new(1, e)),
                            },
                            Err(e) => RResult::RErr(ServiceError::new(1, e)),
                        }
                    } else {
                        // Interactive create
                        if let Err(e) = interactive::run_interactive(&manager) {
                            return RResult::RErr(ServiceError::new(1, e));
                        }
                        RResult::ROk(RString::from("Done"))
                    }
                }

                // Run natively in foreground
                "run" => {
                    std::thread::spawn(|| {
                        let rt = match tokio::runtime::Runtime::new() {
                            Ok(rt) => rt,
                            Err(e) => {
                                eprintln!("Failed to create runtime: {}", e);
                                return;
                            }
                        };

                        rt.block_on(async {
                            if let Err(e) = core::run().await {
                                eprintln!("Cocoon error: {}", e);
                            }
                        });
                    });

                    RResult::ROk(RString::from("Cocoon worker started in background"))
                }

                // Help
                "help" | "-h" | "--help" => {
                    let help_text = get_help_text();
                    RResult::ROk(RString::from(help_text))
                }

                // Unknown command
                _ => {
                    println!("Unknown command: {}", subcommand);
                    println!("Run 'adi cocoon help' for usage information.");
                    RResult::RErr(ServiceError::new(
                        1,
                        format!("Unknown command: {}", subcommand),
                    ))
                }
            }
        }
        "list_commands" => {
            let commands = serde_json::json!([
                {"name": "", "description": "Interactive mode (default)", "usage": ""},
                {"name": "list", "description": "List all cocoons", "usage": "list"},
                {"name": "status", "description": "Show cocoon status", "usage": "status <name>"},
                {"name": "start", "description": "Start a cocoon", "usage": "start <name>"},
                {"name": "stop", "description": "Stop a cocoon", "usage": "stop <name>"},
                {"name": "restart", "description": "Restart a cocoon", "usage": "restart <name>"},
                {"name": "logs", "description": "View cocoon logs", "usage": "logs <name> [-f]"},
                {"name": "rm", "description": "Remove a cocoon", "usage": "rm <name> [--force]"},
                {"name": "create", "description": "Create a new cocoon", "usage": "create [--runtime docker|machine] [--name NAME] [--url URL]"},
                {"name": "run", "description": "Run cocoon natively in foreground", "usage": "run"},
                {"name": "help", "description": "Show help", "usage": "help"}
            ]);
            RResult::ROk(RString::from(
                serde_json::to_string(&commands).unwrap_or_default(),
            ))
        }
        _ => RResult::RErr(ServiceError::method_not_found(method.as_str())),
    }
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

    docker_cmd.arg("ghcr.io/adi-family/cocoon:latest");

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
    help                Show this help message

CREATE OPTIONS:
    --runtime TYPE      Runtime: docker or machine
    --name NAME         Container name (docker only)
    --url URL           Signaling server URL
    --token TOKEN       Setup token for auto-claim
    --secret SECRET     Pre-generated secret
    --start             Start service after create (machine only)

RUNTIMES:
    docker      Docker containers (prefix: cocoon-*)
    machine     Native systemd/launchd service

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

ENVIRONMENT VARIABLES:
    SIGNALING_SERVER_URL    WebSocket URL (default: ws://localhost:8080/ws)
    COCOON_SECRET           Pre-generated secret for persistent device ID
    COCOON_SETUP_TOKEN      Setup token for auto-claim
"#
}

extern "C" fn cli_list_methods(_handle: *const c_void) -> RVec<ServiceMethod> {
    vec![
        ServiceMethod::new("run_command").with_description("Run a CLI command"),
        ServiceMethod::new("list_commands").with_description("List available commands"),
    ]
    .into_iter()
    .collect()
}

static CLI_SERVICE_VTABLE: ServiceVTable = ServiceVTable {
    invoke: cli_invoke,
    list_methods: cli_list_methods,
};

// === Plugin VTable Implementation ===

extern "C" fn plugin_info() -> PluginInfo {
    PluginInfo::new("adi.cocoon", "Cocoon", env!("CARGO_PKG_VERSION"), "core")
        .with_author("ADI Team")
        .with_description("Remote containerized worker with PTY support")
        .with_min_host_version("0.8.0")
}

extern "C" fn plugin_init(ctx: *mut PluginContext) -> i32 {
    unsafe {
        let host = (*ctx).host();

        let cli_descriptor =
            ServiceDescriptor::new(SERVICE_CLI, ServiceVersion::new(1, 0, 0), "adi.cocoon")
                .with_description("CLI commands for cocoon worker");

        let cli_handle = ServiceHandle::new(
            SERVICE_CLI,
            ctx as *const c_void,
            &CLI_SERVICE_VTABLE as *const ServiceVTable,
        );

        if let Err(code) = host.register_svc(cli_descriptor, cli_handle) {
            host.error(&format!(
                "Failed to register CLI commands service: {}",
                code
            ));
            return code;
        }

        host.info("Cocoon plugin initialized");
    }

    0
}

extern "C" fn plugin_cleanup(_ctx: *mut PluginContext) {}

// === Plugin Entry Point ===

static PLUGIN_VTABLE: PluginVTable = PluginVTable {
    info: plugin_info,
    init: plugin_init,
    update: ROption::RNone,
    cleanup: plugin_cleanup,
    handle_message: ROption::RNone,
};

#[no_mangle]
pub extern "C" fn plugin_entry() -> *const PluginVTable {
    &PLUGIN_VTABLE
}
