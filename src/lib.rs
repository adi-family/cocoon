//! Cocoon Plugin
//!
//! Remote containerized worker with PTY support and signaling server connectivity.

mod core;

pub use core::run;

use abi_stable::std_types::{ROption, RResult, RStr, RString, RVec};
use lib_plugin_abi::{
    PluginContext, PluginInfo, PluginVTable, ServiceDescriptor, ServiceError,
    ServiceHandle, ServiceMethod, ServiceVTable, ServiceVersion,
};
use std::ffi::c_void;
use base64::Engine;

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
    std::env::current_exe()
        .map_err(|e| format!("Failed to get current binary path: {}", e))
}

fn generate_secret() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..36).map(|_| rng.gen()).collect();
    base64::engine::general_purpose::STANDARD.encode(&bytes)
}

fn service_install() -> Result<String, String> {
    let os = detect_os();

    match os {
        "linux" => install_systemd_service(),
        "macos" => install_launchd_service(),
        _ => Err("Unsupported OS for service installation".to_string()),
    }
}

fn install_systemd_service() -> Result<String, String> {
    // Get required config
    let signaling_url = std::env::var("SIGNALING_SERVER_URL")
        .unwrap_or_else(|_| "ws://localhost:8080/ws".to_string());

    let home_dir = std::env::var("HOME")
        .map_err(|_| "HOME environment variable not set".to_string())?;

    let service_dir = format!("{}/.config/systemd/user", home_dir);
    let service_file = format!("{}/cocoon.service", service_dir);

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&service_dir)
        .map_err(|e| format!("Failed to create service directory: {}", e))?;

    // Generate secret if not exists
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

        // Set permissions to 600
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&secret_file, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("Failed to set secret permissions: {}", e))?;
        }

        new_secret
    };

    // Get setup token if exists
    let setup_token = std::env::var("COCOON_SETUP_TOKEN").ok();

    let binary_path = get_binary_path()?;

    // Create service file content
    let mut service_content = format!(r#"[Unit]
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

    service_content.push_str(r#"
[Install]
WantedBy=default.target
"#);

    // Write service file
    std::fs::write(&service_file, service_content)
        .map_err(|e| format!("Failed to write service file: {}", e))?;

    // Reload systemd and enable service
    std::process::Command::new("systemctl")
        .args(&["--user", "daemon-reload"])
        .status()
        .map_err(|e| format!("Failed to reload systemd: {}", e))?;

    std::process::Command::new("systemctl")
        .args(&["--user", "enable", "cocoon"])
        .status()
        .map_err(|e| format!("Failed to enable service: {}", e))?;

    let msg = format!(
        "✅ Systemd service installed\n\nService file: {}\nSecret file: {}\n\nStart service:\n  systemctl --user start cocoon\n\nCheck status:\n  systemctl --user status cocoon\n\nView logs:\n  journalctl --user -u cocoon -f",
        service_file,
        secret_file
    );

    Ok(msg)
}

fn install_launchd_service() -> Result<String, String> {
    let signaling_url = std::env::var("SIGNALING_SERVER_URL")
        .unwrap_or_else(|_| "ws://localhost:8080/ws".to_string());

    let home_dir = std::env::var("HOME")
        .map_err(|_| "HOME environment variable not set".to_string())?;

    let plist_dir = format!("{}/Library/LaunchAgents", home_dir);
    let plist_file = format!("{}/com.adi.cocoon.plist", plist_dir);

    std::fs::create_dir_all(&plist_dir)
        .map_err(|e| format!("Failed to create LaunchAgents directory: {}", e))?;

    // Generate secret
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

    let mut plist_content = format!(r#"<?xml version="1.0" encoding="UTF-8"?>
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
        plist_content.push_str(&format!(r#"        <key>COCOON_SETUP_TOKEN</key>
        <string>{}</string>
"#, token));
    }

    plist_content.push_str(r#"    </dict>
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
"#);

    std::fs::write(&plist_file, plist_content)
        .map_err(|e| format!("Failed to write plist file: {}", e))?;

    let msg = format!(
        "✅ LaunchAgent plist created\n\nPlist file: {}\nSecret file: {}\n\nLoad service:\n  launchctl load {}\n\nCheck status:\n  launchctl list | grep cocoon\n\nView logs:\n  tail -f /tmp/cocoon.log",
        plist_file,
        secret_file,
        plist_file
    );

    Ok(msg)
}

fn service_start() -> Result<String, String> {
    let os = detect_os();

    match os {
        "linux" => {
            let output = std::process::Command::new("systemctl")
                .args(&["--user", "start", "cocoon"])
                .output()
                .map_err(|e| format!("Failed to start service: {}", e))?;

            if output.status.success() {
                Ok("✅ Service started\n\nCheck status: systemctl --user status cocoon".to_string())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(format!("Failed to start service: {}", stderr))
            }
        }
        "macos" => {
            let home_dir = std::env::var("HOME")
                .map_err(|_| "HOME not set".to_string())?;
            let plist_file = format!("{}/Library/LaunchAgents/com.adi.cocoon.plist", home_dir);

            let output = std::process::Command::new("launchctl")
                .args(&["load", &plist_file])
                .output()
                .map_err(|e| format!("Failed to load service: {}", e))?;

            if output.status.success() {
                Ok("✅ Service loaded\n\nCheck status: launchctl list | grep cocoon".to_string())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(format!("Failed to load service: {}", stderr))
            }
        }
        _ => Err("Unsupported OS".to_string()),
    }
}

fn service_stop() -> Result<String, String> {
    let os = detect_os();

    match os {
        "linux" => {
            let output = std::process::Command::new("systemctl")
                .args(&["--user", "stop", "cocoon"])
                .output()
                .map_err(|e| format!("Failed to stop service: {}", e))?;

            if output.status.success() {
                Ok("✅ Service stopped".to_string())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(format!("Failed to stop service: {}", stderr))
            }
        }
        "macos" => {
            let home_dir = std::env::var("HOME")
                .map_err(|_| "HOME not set".to_string())?;
            let plist_file = format!("{}/Library/LaunchAgents/com.adi.cocoon.plist", home_dir);

            let output = std::process::Command::new("launchctl")
                .args(&["unload", &plist_file])
                .output()
                .map_err(|e| format!("Failed to unload service: {}", e))?;

            if output.status.success() {
                Ok("✅ Service unloaded".to_string())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(format!("Failed to unload service: {}", stderr))
            }
        }
        _ => Err("Unsupported OS".to_string()),
    }
}

fn service_restart() -> Result<String, String> {
    service_stop()?;
    std::thread::sleep(std::time::Duration::from_secs(1));
    service_start()
}

fn service_status() -> Result<String, String> {
    let os = detect_os();

    match os {
        "linux" => {
            let output = std::process::Command::new("systemctl")
                .args(&["--user", "status", "cocoon"])
                .output()
                .map_err(|e| format!("Failed to get status: {}", e))?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(stdout.to_string())
        }
        "macos" => {
            let output = std::process::Command::new("launchctl")
                .args(&["list"])
                .output()
                .map_err(|e| format!("Failed to get status: {}", e))?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let cocoon_status: Vec<_> = stdout.lines()
                .filter(|line| line.contains("cocoon"))
                .collect();

            if cocoon_status.is_empty() {
                Ok("Service not running".to_string())
            } else {
                Ok(cocoon_status.join("\n"))
            }
        }
        _ => Err("Unsupported OS".to_string()),
    }
}

fn service_logs() -> Result<String, String> {
    let os = detect_os();

    match os {
        "linux" => {
            println!("Following logs (Ctrl+C to stop)...");
            let status = std::process::Command::new("journalctl")
                .args(&["--user", "-u", "cocoon", "-f"])
                .status()
                .map_err(|e| format!("Failed to view logs: {}", e))?;

            if status.success() {
                Ok("".to_string())
            } else {
                Err("Failed to view logs".to_string())
            }
        }
        "macos" => {
            println!("Following logs (Ctrl+C to stop)...");
            let status = std::process::Command::new("tail")
                .args(&["-f", "/tmp/cocoon.log"])
                .status()
                .map_err(|e| format!("Failed to view logs: {}", e))?;

            if status.success() {
                Ok("".to_string())
            } else {
                Err("Failed to view logs".to_string())
            }
        }
        _ => Err("Unsupported OS".to_string()),
    }
}

fn service_uninstall() -> Result<String, String> {
    // Stop service first
    let _ = service_stop();

    let os = detect_os();

    match os {
        "linux" => {
            let home_dir = std::env::var("HOME")
                .map_err(|_| "HOME not set".to_string())?;
            let service_file = format!("{}/.config/systemd/user/cocoon.service", home_dir);

            std::process::Command::new("systemctl")
                .args(&["--user", "disable", "cocoon"])
                .status()
                .ok();

            if std::path::Path::new(&service_file).exists() {
                std::fs::remove_file(&service_file)
                    .map_err(|e| format!("Failed to remove service file: {}", e))?;
            }

            std::process::Command::new("systemctl")
                .args(&["--user", "daemon-reload"])
                .status()
                .ok();

            Ok("✅ Service uninstalled".to_string())
        }
        "macos" => {
            let home_dir = std::env::var("HOME")
                .map_err(|_| "HOME not set".to_string())?;
            let plist_file = format!("{}/Library/LaunchAgents/com.adi.cocoon.plist", home_dir);

            if std::path::Path::new(&plist_file).exists() {
                std::fs::remove_file(&plist_file)
                    .map_err(|e| format!("Failed to remove plist: {}", e))?;
            }

            Ok("✅ Service uninstalled".to_string())
        }
        _ => Err("Unsupported OS".to_string()),
    }
}

// === CLI Service Implementation ===

extern "C" fn cli_invoke(
    _handle: *const c_void,
    method: RStr<'_>,
    args: RStr<'_>,
) -> RResult<RString, ServiceError> {
    match method.as_str() {
        "run_command" => {
            // Parse the args JSON (context from CLI host)
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

            let subcommand = cmd_args.first().map(|s| s.as_str()).unwrap_or("run");

            match subcommand {
                "start" => {
                    let mode = cmd_args.get(1).map(|s| s.as_str()).unwrap_or("native");

                    match mode {
                        "docker" => {
                            // Parse CLI flags
                            let signaling_url = cmd_args.iter()
                                .position(|arg| arg == "--url")
                                .and_then(|idx| cmd_args.get(idx + 1))
                                .map(|s| s.to_string())
                                .or_else(|| std::env::var("SIGNALING_SERVER_URL").ok())
                                .unwrap_or_else(|| "ws://localhost:8080/ws".to_string());

                            let cocoon_secret = cmd_args.iter()
                                .position(|arg| arg == "--secret")
                                .and_then(|idx| cmd_args.get(idx + 1))
                                .map(|s| s.to_string())
                                .or_else(|| std::env::var("COCOON_SECRET").ok());

                            let setup_token = cmd_args.iter()
                                .position(|arg| arg == "--token")
                                .and_then(|idx| cmd_args.get(idx + 1))
                                .map(|s| s.to_string())
                                .or_else(|| std::env::var("COCOON_SETUP_TOKEN").ok());

                            let cocoon_name = cmd_args.iter()
                                .position(|arg| arg == "--name")
                                .and_then(|idx| cmd_args.get(idx + 1))
                                .map(|s| s.to_string())
                                .or_else(|| std::env::var("COCOON_NAME").ok())
                                .unwrap_or_else(|| {
                                    // Check existing containers
                                    let output = std::process::Command::new("docker")
                                        .args(&["ps", "-a", "--format", "{{.Names}}"])
                                        .output();

                                    if let Ok(output) = output {
                                        let names = String::from_utf8_lossy(&output.stdout);
                                        let existing: Vec<&str> = names.lines()
                                            .filter(|n| n.starts_with("cocoon-"))
                                            .collect();

                                        if existing.is_empty() {
                                            "cocoon-worker".to_string()
                                        } else if existing.contains(&"cocoon-worker") {
                                            // Find next available number
                                            let mut num = 2;
                                            loop {
                                                let candidate = format!("cocoon-worker-{}", num);
                                                if !existing.contains(&candidate.as_str()) {
                                                    println!("📝 Auto-generated container name: {}", candidate);
                                                    return candidate;
                                                }
                                                num += 1;
                                            }
                                        } else {
                                            "cocoon-worker".to_string()
                                        }
                                    } else {
                                        "cocoon-worker".to_string()
                                    }
                                });

                            // Build docker run command for PRODUCTION (daemon mode)
                            let mut docker_cmd = std::process::Command::new("docker");
                            docker_cmd.arg("run")
                                .arg("-d")                              // Daemon mode
                                .arg("--restart").arg("unless-stopped")  // Auto-restart
                                .arg("--name").arg(&cocoon_name)         // Named container (customizable)
                                .arg("-e").arg(format!("SIGNALING_SERVER_URL={}", signaling_url))
                                .arg("-v").arg(format!("{}:/cocoon", cocoon_name));

                            if let Some(secret) = cocoon_secret {
                                docker_cmd.arg("-e").arg(format!("COCOON_SECRET={}", secret));
                            }

                            if let Some(token) = setup_token {
                                docker_cmd.arg("-e").arg(format!("COCOON_SETUP_TOKEN={}", token));
                            }

                            docker_cmd.arg("ghcr.io/adi-family/cocoon:latest");

                            println!("Starting cocoon in Docker container (daemon mode)...");

                            match docker_cmd.output() {
                                Ok(output) if output.status.success() => {
                                    let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
                                    println!("✅ Cocoon container started: {}", container_id);
                                    println!();
                                    println!("Container name: {}", cocoon_name);
                                    println!();
                                    println!("Check status:");
                                    println!("  docker ps | grep {}", cocoon_name);
                                    println!("  docker logs -f {}", cocoon_name);
                                    println!();
                                    println!("Stop container:");
                                    println!("  docker stop {}", cocoon_name);

                                    RResult::ROk(RString::from(format!("Container '{}' started: {}", cocoon_name, container_id)))
                                }
                                Ok(output) => {
                                    let stderr = String::from_utf8_lossy(&output.stderr);
                                    RResult::RErr(ServiceError::new(1, format!("Docker failed: {}", stderr)))
                                }
                                Err(e) => {
                                    RResult::RErr(ServiceError::new(1, format!("Failed to start Docker: {}. Make sure Docker is installed and running.", e)))
                                }
                            }
                        }
                        "native" | _ => {
                            // Start cocoon worker in a background task
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
                    }
                }
                "run" => {
                    // Alias for "start native"
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
                "service" => {
                    let action = cmd_args.get(1).map(|s| s.as_str()).unwrap_or("help");

                    match action {
                        "install" => match service_install() {
                            Ok(msg) => RResult::ROk(RString::from(msg)),
                            Err(e) => RResult::RErr(ServiceError::new(1, e)),
                        },
                        "uninstall" => match service_uninstall() {
                            Ok(msg) => RResult::ROk(RString::from(msg)),
                            Err(e) => RResult::RErr(ServiceError::new(1, e)),
                        },
                        "start" => match service_start() {
                            Ok(msg) => RResult::ROk(RString::from(msg)),
                            Err(e) => RResult::RErr(ServiceError::new(1, e)),
                        },
                        "stop" => match service_stop() {
                            Ok(msg) => RResult::ROk(RString::from(msg)),
                            Err(e) => RResult::RErr(ServiceError::new(1, e)),
                        },
                        "restart" => match service_restart() {
                            Ok(msg) => RResult::ROk(RString::from(msg)),
                            Err(e) => RResult::RErr(ServiceError::new(1, e)),
                        },
                        "status" => match service_status() {
                            Ok(msg) => RResult::ROk(RString::from(msg)),
                            Err(e) => RResult::RErr(ServiceError::new(1, e)),
                        },
                        "logs" => match service_logs() {
                            Ok(msg) => RResult::ROk(RString::from(msg)),
                            Err(e) => RResult::RErr(ServiceError::new(1, e)),
                        },
                        _ => {
                            let help = r#"Service management commands:

USAGE:
    adi cocoon service [ACTION]

ACTIONS:
    install     Install systemd/launchd service
    uninstall   Remove systemd/launchd service
    start       Start the cocoon service
    stop        Stop the cocoon service
    restart     Restart the cocoon service
    status      Show service status
    logs        Show service logs (follow mode)

EXAMPLES:
    adi cocoon service install
    adi cocoon service start
    adi cocoon service status
    adi cocoon service logs
"#;
                            RResult::ROk(RString::from(help))
                        }
                    }
                }
                "help" | _ => {
                    let help_text = r#"Cocoon - Remote containerized worker

USAGE:
    adi cocoon [COMMAND]

COMMANDS:
    run                         Start the cocoon worker natively in foreground
    start [MODE] [OPTIONS]      Start cocoon worker
        native                  Start natively in background
        docker [OPTIONS]        Start in Docker container (daemon mode)
                                --name NAME       Container name (default: auto-generated)
                                --url URL         Signaling server URL
                                --token TOKEN     Setup token for auto-claim
                                --secret SECRET   Pre-generated secret for device ID
    service                     Manage cocoon as a system service
        install         Install systemd/launchd service
        uninstall       Remove service
        start           Start service
        stop            Stop service
        restart         Restart service
        status          Show service status
        logs            Follow service logs
    help                Show this help message

ENVIRONMENT VARIABLES:
    SIGNALING_SERVER_URL    WebSocket URL (default: ws://localhost:8080/ws)
    COCOON_SECRET           Pre-generated secret for persistent device ID
    COCOON_SETUP_TOKEN      Setup token for auto-claim
    COCOON_NAME             Optional name for this cocoon instance

EXAMPLES:
    # Quick test (foreground)
    adi cocoon run

    # Production setup (systemd/launchd)
    adi cocoon service install
    adi cocoon service start
    adi cocoon service status

    # Docker daemon (minimal - auto-named, localhost)
    adi cocoon start docker

    # Docker daemon (with custom name)
    adi cocoon start docker --name my-worker

    # Docker daemon (production with all flags)
    adi cocoon start docker \
      --name prod-worker \
      --url wss://adi.the-ihor.com/api/signaling/ws \
      --token <your-setup-token>

    # Multiple instances
    adi cocoon start docker --name worker-1 --url wss://example.com/ws
    adi cocoon start docker --name worker-2 --url wss://example.com/ws
    adi cocoon start docker --name worker-3 --url wss://example.com/ws
    docker logs -f worker-1

PRODUCTION DEPLOYMENT:
    For production, use either:
    1. System service (recommended for VPS/servers)
       adi cocoon service install && adi cocoon service start

    2. Docker daemon (recommended for containers)
       adi cocoon start docker
"#;
                    RResult::ROk(RString::from(help_text))
                }
            }
        }
        "list_commands" => {
            let commands = serde_json::json!([
                {"name": "run", "description": "Start the cocoon worker natively (foreground)", "usage": "run"},
                {"name": "start", "description": "Start cocoon worker (native or docker daemon)", "usage": "start [native|docker] [--name NAME] [--url URL] [--token TOKEN] [--secret SECRET]"},
                {"name": "service", "description": "Manage cocoon as a system service", "usage": "service [install|start|stop|status|logs|uninstall]"},
                {"name": "help", "description": "Show help message", "usage": "help"}
            ]);
            RResult::ROk(RString::from(
                serde_json::to_string(&commands).unwrap_or_default(),
            ))
        }
        _ => RResult::RErr(ServiceError::method_not_found(method.as_str())),
    }
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

        // Register CLI commands service
        let cli_descriptor = ServiceDescriptor::new(
            SERVICE_CLI,
            ServiceVersion::new(1, 0, 0),
            "adi.cocoon",
        )
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
