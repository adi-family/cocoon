//! Interactive CLI mode using inquire.

use crate::runtime::{CocoonInfo, RuntimeManager, RuntimeType};
use inquire::{Confirm, Select, Text};

/// Actions available in interactive mode
#[derive(Debug, Clone)]
pub enum InteractiveAction {
    List,
    Status,
    Start,
    Stop,
    Restart,
    Logs,
    Update,
    CheckUpdate,
    Remove,
    Create,
    Help,
    Exit,
}

impl std::fmt::Display for InteractiveAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InteractiveAction::List => write!(f, "list     - List all cocoons"),
            InteractiveAction::Status => write!(f, "status   - Show cocoon status"),
            InteractiveAction::Start => write!(f, "start    - Start a cocoon"),
            InteractiveAction::Stop => write!(f, "stop     - Stop a cocoon"),
            InteractiveAction::Restart => write!(f, "restart  - Restart a cocoon"),
            InteractiveAction::Logs => write!(f, "logs     - View cocoon logs"),
            InteractiveAction::Update => write!(f, "update   - Update cocoon to latest version"),
            InteractiveAction::CheckUpdate => write!(f, "check    - Check for available updates"),
            InteractiveAction::Remove => write!(f, "remove   - Remove a cocoon"),
            InteractiveAction::Create => write!(f, "create   - Create a new cocoon"),
            InteractiveAction::Help => write!(f, "help     - Show help"),
            InteractiveAction::Exit => write!(f, "exit     - Exit interactive mode"),
        }
    }
}

/// Run interactive mode
pub fn run_interactive(manager: &RuntimeManager) -> Result<(), String> {
    println!("\nCocoon Interactive Mode");
    println!("=======================\n");

    loop {
        let actions = vec![
            InteractiveAction::List,
            InteractiveAction::Status,
            InteractiveAction::Start,
            InteractiveAction::Stop,
            InteractiveAction::Restart,
            InteractiveAction::Logs,
            InteractiveAction::Update,
            InteractiveAction::CheckUpdate,
            InteractiveAction::Remove,
            InteractiveAction::Create,
            InteractiveAction::Help,
            InteractiveAction::Exit,
        ];

        let selection = Select::new("Select action:", actions)
            .with_help_message("Use arrow keys to navigate, Enter to select")
            .prompt();

        match selection {
            Ok(InteractiveAction::List) => {
                if let Err(e) = handle_list(manager) {
                    println!("Error: {}\n", e);
                }
            }
            Ok(InteractiveAction::Status) => {
                if let Err(e) = handle_status_interactive(manager) {
                    println!("Error: {}\n", e);
                }
            }
            Ok(InteractiveAction::Start) => {
                if let Err(e) = handle_start_interactive(manager) {
                    println!("Error: {}\n", e);
                }
            }
            Ok(InteractiveAction::Stop) => {
                if let Err(e) = handle_stop_interactive(manager) {
                    println!("Error: {}\n", e);
                }
            }
            Ok(InteractiveAction::Restart) => {
                if let Err(e) = handle_restart_interactive(manager) {
                    println!("Error: {}\n", e);
                }
            }
            Ok(InteractiveAction::Logs) => {
                if let Err(e) = handle_logs_interactive(manager) {
                    println!("Error: {}\n", e);
                }
            }
            Ok(InteractiveAction::Update) => {
                if let Err(e) = handle_update_interactive(manager) {
                    println!("Error: {}\n", e);
                }
            }
            Ok(InteractiveAction::CheckUpdate) => {
                if let Err(e) = handle_check_update_interactive(manager) {
                    println!("Error: {}\n", e);
                }
            }
            Ok(InteractiveAction::Remove) => {
                if let Err(e) = handle_remove_interactive(manager) {
                    println!("Error: {}\n", e);
                }
            }
            Ok(InteractiveAction::Create) => {
                if let Err(e) = handle_create_interactive(manager) {
                    println!("Error: {}\n", e);
                }
            }
            Ok(InteractiveAction::Help) => {
                print_help();
            }
            Ok(InteractiveAction::Exit) => {
                println!("Goodbye!");
                break;
            }
            Err(_) => {
                println!("Cancelled");
                break;
            }
        }
    }

    Ok(())
}

/// Format cocoon info for display
fn format_cocoon(info: &CocoonInfo) -> String {
    let reset = "\x1b[0m";
    format!(
        "{}{}{} {} [{}]",
        info.status_color(),
        info.status_icon(),
        reset,
        info.name,
        info.runtime
    )
}

/// Handle list command
pub fn handle_list(manager: &RuntimeManager) -> Result<(), String> {
    let cocoons = manager.list_all()?;

    if cocoons.is_empty() {
        println!("\nNo cocoons found.");
        println!("Create one with: adi cocoon create\n");
        return Ok(());
    }

    println!("\n{:<20} {:<10} {:<10}", "NAME", "RUNTIME", "STATUS");
    println!("{}", "-".repeat(42));

    let reset = "\x1b[0m";
    for cocoon in &cocoons {
        println!(
            "{:<20} {:<10} {}{}{}{}",
            cocoon.name,
            cocoon.runtime,
            cocoon.status_color(),
            cocoon.status_icon(),
            cocoon.status,
            reset
        );
    }
    println!();

    Ok(())
}

/// Select a cocoon interactively
fn select_cocoon(manager: &RuntimeManager, prompt: &str) -> Result<CocoonInfo, String> {
    let cocoons = manager.list_all()?;

    if cocoons.is_empty() {
        return Err("No cocoons found. Create one with: adi cocoon create".to_string());
    }

    let display_names: Vec<String> = cocoons.iter().map(format_cocoon).collect();

    let selection = Select::new(prompt, display_names)
        .prompt()
        .map_err(|_| "Selection cancelled".to_string())?;

    // Find the selected cocoon
    let idx = cocoons
        .iter()
        .position(|c| format_cocoon(c) == selection)
        .ok_or("Cocoon not found")?;

    Ok(cocoons[idx].clone())
}

/// Handle status command interactively
fn handle_status_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to check status:")?;
    let runtime = manager.get_runtime(cocoon.runtime);
    let info = runtime.status(&cocoon.name)?;

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

    Ok(())
}

/// Handle start command interactively
fn handle_start_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to start:")?;
    let runtime = manager.get_runtime(cocoon.runtime);

    println!("Starting '{}'...", cocoon.name);
    let result = runtime.start(&cocoon.name)?;
    println!("{}\n", result);

    Ok(())
}

/// Handle stop command interactively
fn handle_stop_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to stop:")?;
    let runtime = manager.get_runtime(cocoon.runtime);

    println!("Stopping '{}'...", cocoon.name);
    let result = runtime.stop(&cocoon.name)?;
    println!("{}\n", result);

    Ok(())
}

/// Handle restart command interactively
fn handle_restart_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to restart:")?;
    let runtime = manager.get_runtime(cocoon.runtime);

    println!("Restarting '{}'...", cocoon.name);
    let result = runtime.restart(&cocoon.name)?;
    println!("{}\n", result);

    Ok(())
}

/// Handle logs command interactively
fn handle_logs_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to view logs:")?;
    let runtime = manager.get_runtime(cocoon.runtime);

    let follow = Confirm::new("Follow logs?")
        .with_default(true)
        .prompt()
        .unwrap_or(false);

    runtime.logs(&cocoon.name, follow, Some(50))?;
    println!();

    Ok(())
}

/// Handle update command interactively
fn handle_update_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to update:")?;
    let runtime = manager.get_runtime(cocoon.runtime);

    let confirm = Confirm::new(&format!(
        "Update cocoon '{}'? This will restart the cocoon.",
        cocoon.name
    ))
    .with_default(true)
    .prompt()
    .unwrap_or(false);

    if !confirm {
        println!("Cancelled\n");
        return Ok(());
    }

    println!("\nUpdating '{}'...\n", cocoon.name);
    let result = runtime.update(&cocoon.name)?;
    println!("{}\n", result);

    Ok(())
}

/// Handle check-update command interactively
fn handle_check_update_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to check for updates:")?;
    let runtime = manager.get_runtime(cocoon.runtime);

    let result = runtime.check_update(&cocoon.name)?;
    println!("{}", result);

    Ok(())
}

/// Handle remove command interactively
fn handle_remove_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to remove:")?;
    let runtime = manager.get_runtime(cocoon.runtime);

    let confirm = Confirm::new(&format!("Remove cocoon '{}'?", cocoon.name))
        .with_default(false)
        .prompt()
        .unwrap_or(false);

    if !confirm {
        println!("Cancelled\n");
        return Ok(());
    }

    let force = matches!(cocoon.status, crate::runtime::CocoonStatus::Running);
    if force {
        println!("Cocoon is running, will force stop first...");
    }

    println!("Removing '{}'...", cocoon.name);
    let result = runtime.remove(&cocoon.name, force)?;
    println!("{}\n", result);

    Ok(())
}

/// Handle create command interactively
fn handle_create_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let runtimes = manager.available_runtimes();

    if runtimes.is_empty() {
        return Err(
            "No runtimes available. Install Docker or use a supported OS (Linux/macOS)."
                .to_string(),
        );
    }

    let runtime_names: Vec<String> = runtimes.iter().map(|r| r.to_string()).collect();
    let runtime_selection = Select::new("Select runtime:", runtime_names)
        .with_help_message("docker = container, machine = native service")
        .prompt()
        .map_err(|_| "Selection cancelled".to_string())?;

    let runtime_type = RuntimeType::from_str(&runtime_selection).ok_or("Invalid runtime")?;

    match runtime_type {
        RuntimeType::Docker => create_docker_cocoon_interactive(),
        RuntimeType::Machine => create_machine_cocoon_interactive(),
    }
}

/// Create a Docker cocoon interactively
fn create_docker_cocoon_interactive() -> Result<(), String> {
    let name = Text::new("Container name:")
        .with_default("cocoon-worker")
        .with_help_message("Name for the Docker container")
        .prompt()
        .map_err(|_| "Cancelled".to_string())?;

    let signaling_url = Text::new("Signaling server URL:")
        .with_default("ws://localhost:8080/ws")
        .with_help_message("WebSocket URL for the signaling server")
        .prompt()
        .map_err(|_| "Cancelled".to_string())?;

    let setup_token = Text::new("Setup token (optional):")
        .with_help_message("Token for auto-claim, leave empty to skip")
        .prompt()
        .map_err(|_| "Cancelled".to_string())?;

    // Build docker run command
    let mut docker_cmd = std::process::Command::new("docker");
    docker_cmd
        .arg("run")
        .arg("-d")
        .arg("--restart")
        .arg("unless-stopped")
        .arg("--name")
        .arg(&name);

    // Add host mapping for .local domains
    if let Ok(url) = url::Url::parse(&signaling_url) {
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

    if !setup_token.is_empty() {
        docker_cmd
            .arg("-e")
            .arg(format!("COCOON_SETUP_TOKEN={}", setup_token));
    }

    docker_cmd.arg("ghcr.io/adi-family/cocoon:latest");

    println!("\nCreating Docker cocoon '{}'...", name);

    match docker_cmd.output() {
        Ok(output) if output.status.success() => {
            let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            println!("Container created: {}", container_id);
            println!("\nView logs: adi cocoon logs {}", name);
            println!("Stop: adi cocoon stop {}", name);
            println!();
            Ok(())
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("Docker failed: {}", stderr))
        }
        Err(e) => Err(format!("Failed to start Docker: {}", e)),
    }
}

/// Create a Machine (native service) cocoon interactively
fn create_machine_cocoon_interactive() -> Result<(), String> {
    let signaling_url = Text::new("Signaling server URL:")
        .with_default("ws://localhost:8080/ws")
        .prompt()
        .map_err(|_| "Cancelled".to_string())?;

    // Set env var for install function
    std::env::set_var("SIGNALING_SERVER_URL", &signaling_url);

    let setup_token = Text::new("Setup token (optional):")
        .prompt()
        .map_err(|_| "Cancelled".to_string())?;

    if !setup_token.is_empty() {
        std::env::set_var("COCOON_SETUP_TOKEN", &setup_token);
    }

    println!("\nInstalling machine service...");

    // Use the existing service_install function
    let result = crate::service_install()?;
    println!("{}", result);

    let start = Confirm::new("Start service now?")
        .with_default(true)
        .prompt()
        .unwrap_or(false);

    if start {
        let start_result = crate::service_start()?;
        println!("{}", start_result);
    }

    println!();
    Ok(())
}

/// Print help text
fn print_help() {
    println!(
        r#"
Cocoon Interactive Mode - Help
==============================

Commands:
  list     - List all cocoons across all runtimes
  status   - Show detailed status for a specific cocoon
  start    - Start a stopped cocoon
  stop     - Stop a running cocoon
  restart  - Restart a cocoon
  logs     - View logs for a cocoon
  update   - Update cocoon to latest version
  check    - Check for available updates
  remove   - Remove a cocoon (stops if running)
  create   - Create a new cocoon (Docker or Machine)
  help     - Show this help
  exit     - Exit interactive mode

Runtimes:
  docker   - Docker containers (cocoon-* prefix)
             Update: Pulls latest image and recreates container
  machine  - Native systemd/launchd service
             Update: Downloads latest binary and restarts service

Tips:
  - Use arrow keys to navigate menus
  - Press Enter to select
  - Press Ctrl+C to cancel

Non-interactive usage:
  adi cocoon list
  adi cocoon start <name>
  adi cocoon stop <name>
  adi cocoon logs <name> [-f]
  adi cocoon update <name>
  adi cocoon check-update <name>

"#
    );
}
