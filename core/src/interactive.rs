//! Interactive CLI mode.

use crate::runtime::{CocoonInfo, CocoonStatus, RuntimeManager, RuntimeType};
use lib_console_output::{
    out_error, out_info, out_success, out_warn, theme, Columns, Confirm, Input, KeyValue, List,
    Renderable, Section, Select, SelectOption,
};

#[derive(Debug, Clone)]
enum InteractiveAction {
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

fn action_options() -> Vec<SelectOption<InteractiveAction>> {
    vec![
        SelectOption::new("list", InteractiveAction::List)
            .with_description("List all cocoons"),
        SelectOption::new("status", InteractiveAction::Status)
            .with_description("Show cocoon status"),
        SelectOption::new("start", InteractiveAction::Start)
            .with_description("Start a cocoon"),
        SelectOption::new("stop", InteractiveAction::Stop)
            .with_description("Stop a cocoon"),
        SelectOption::new("restart", InteractiveAction::Restart)
            .with_description("Restart a cocoon"),
        SelectOption::new("logs", InteractiveAction::Logs)
            .with_description("View cocoon logs"),
        SelectOption::new("update", InteractiveAction::Update)
            .with_description("Update cocoon to latest version"),
        SelectOption::new("check", InteractiveAction::CheckUpdate)
            .with_description("Check for available updates"),
        SelectOption::new("remove", InteractiveAction::Remove)
            .with_description("Remove a cocoon"),
        SelectOption::new("create", InteractiveAction::Create)
            .with_description("Create a new cocoon"),
        SelectOption::new("help", InteractiveAction::Help)
            .with_description("Show help"),
        SelectOption::new("exit", InteractiveAction::Exit)
            .with_description("Exit interactive mode"),
    ]
}

pub fn run_interactive(manager: &RuntimeManager) -> Result<(), String> {
    out_info!("Cocoon Interactive Mode");

    loop {
        let selection = Select::new("Select action:")
            .options(action_options())
            .run();

        match selection {
            Some(InteractiveAction::List) => {
                if let Err(e) = handle_list(manager) {
                    out_error!("{}", e);
                }
            }
            Some(InteractiveAction::Status) => {
                if let Err(e) = handle_status_interactive(manager) {
                    out_error!("{}", e);
                }
            }
            Some(InteractiveAction::Start) => {
                if let Err(e) = handle_start_interactive(manager) {
                    out_error!("{}", e);
                }
            }
            Some(InteractiveAction::Stop) => {
                if let Err(e) = handle_stop_interactive(manager) {
                    out_error!("{}", e);
                }
            }
            Some(InteractiveAction::Restart) => {
                if let Err(e) = handle_restart_interactive(manager) {
                    out_error!("{}", e);
                }
            }
            Some(InteractiveAction::Logs) => {
                if let Err(e) = handle_logs_interactive(manager) {
                    out_error!("{}", e);
                }
            }
            Some(InteractiveAction::Update) => {
                if let Err(e) = handle_update_interactive(manager) {
                    out_error!("{}", e);
                }
            }
            Some(InteractiveAction::CheckUpdate) => {
                if let Err(e) = handle_check_update_interactive(manager) {
                    out_error!("{}", e);
                }
            }
            Some(InteractiveAction::Remove) => {
                if let Err(e) = handle_remove_interactive(manager) {
                    out_error!("{}", e);
                }
            }
            Some(InteractiveAction::Create) => {
                if let Err(e) = handle_create_interactive(manager) {
                    out_error!("{}", e);
                }
            }
            Some(InteractiveAction::Help) => {
                print_help();
            }
            Some(InteractiveAction::Exit) => {
                out_info!("Goodbye!");
                break;
            }
            None => {
                out_warn!("Cancelled");
                break;
            }
        }
    }

    Ok(())
}

/// Build a SelectOption from a CocoonInfo.
fn cocoon_option(info: &CocoonInfo) -> SelectOption<String> {
    let icon = info.status_icon();
    let styled_icon = match &info.status {
        CocoonStatus::Running => theme::success(icon).to_string(),
        CocoonStatus::Stopped => theme::muted(icon).to_string(),
        CocoonStatus::Restarting => theme::warning(icon).to_string(),
        CocoonStatus::Unknown(_) => theme::error(icon).to_string(),
    };
    let label = format!("{} {} [{}]", styled_icon, info.name, info.runtime);
    SelectOption::new(label, info.name.clone())
}

/// Handle list command
pub fn handle_list(manager: &RuntimeManager) -> Result<(), String> {
    let cocoons = manager.list_all()?;

    if cocoons.is_empty() {
        out_info!("No cocoons found. Create one with: adi cocoon create");
        return Ok(());
    }

    let cols = cocoons.iter().fold(
        Columns::new().header(["NAME", "RUNTIME", "STATUS"]),
        |cols, cocoon| {
            let status_str = format!("{} {}", cocoon.status_icon(), cocoon.status);
            let styled_status = match &cocoon.status {
                CocoonStatus::Running => theme::success(&status_str).to_string(),
                CocoonStatus::Stopped => theme::muted(&status_str).to_string(),
                CocoonStatus::Restarting => theme::warning(&status_str).to_string(),
                CocoonStatus::Unknown(_) => theme::error(&status_str).to_string(),
            };
            cols.row([cocoon.name.clone(), cocoon.runtime.to_string(), styled_status])
        },
    );
    cols.print();

    Ok(())
}

/// Select a cocoon interactively
fn select_cocoon(manager: &RuntimeManager, prompt: &str) -> Result<CocoonInfo, String> {
    let cocoons = manager.list_all()?;

    if cocoons.is_empty() {
        return Err("No cocoons found. Create one with: adi cocoon create".to_string());
    }

    let options: Vec<SelectOption<String>> = cocoons.iter().map(cocoon_option).collect();

    let selected_name = Select::new(prompt)
        .options(options)
        .run()
        .ok_or_else(|| "Selection cancelled".to_string())?;

    cocoons
        .into_iter()
        .find(|c| c.name == selected_name)
        .ok_or_else(|| "Cocoon not found".to_string())
}

/// Handle status command interactively
fn handle_status_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to check status:")?;
    let runtime = manager.get_runtime(cocoon.runtime);
    let info = runtime.status(&cocoon.name)?;

    let status_str = format!("{} {}", info.status_icon(), info.status);
    let styled_status = match &info.status {
        CocoonStatus::Running => theme::success(&status_str).to_string(),
        CocoonStatus::Stopped => theme::muted(&status_str).to_string(),
        CocoonStatus::Restarting => theme::warning(&status_str).to_string(),
        CocoonStatus::Unknown(_) => theme::error(&status_str).to_string(),
    };
    let mut kv = KeyValue::new()
        .entry("Cocoon", &info.name)
        .entry("Runtime", info.runtime.to_string())
        .entry("Status", styled_status);
    if let Some(image) = &info.image {
        kv = kv.entry("Image", image);
    }
    if let Some(created) = &info.created {
        kv = kv.entry("Created", created);
    }
    kv.print();

    Ok(())
}

/// Handle start command interactively
fn handle_start_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to start:")?;
    let runtime = manager.get_runtime(cocoon.runtime);

    out_info!("Starting '{}'...", cocoon.name);
    let result = runtime.start(&cocoon.name)?;
    out_success!("{}", result);

    Ok(())
}

/// Handle stop command interactively
fn handle_stop_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to stop:")?;
    let runtime = manager.get_runtime(cocoon.runtime);

    out_info!("Stopping '{}'...", cocoon.name);
    let result = runtime.stop(&cocoon.name)?;
    out_success!("{}", result);

    Ok(())
}

/// Handle restart command interactively
fn handle_restart_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to restart:")?;
    let runtime = manager.get_runtime(cocoon.runtime);

    out_info!("Restarting '{}'...", cocoon.name);
    let result = runtime.restart(&cocoon.name)?;
    out_success!("{}", result);

    Ok(())
}

/// Handle logs command interactively
fn handle_logs_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to view logs:")?;
    let runtime = manager.get_runtime(cocoon.runtime);

    let follow = Confirm::new("Follow logs?")
        .default(true)
        .run()
        .unwrap_or(false);

    runtime.logs(&cocoon.name, follow, Some(50))?;

    Ok(())
}

/// Handle update command interactively
fn handle_update_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to update:")?;
    let runtime = manager.get_runtime(cocoon.runtime);

    let confirm = Confirm::new(format!(
        "Update cocoon '{}'? This will restart the cocoon.",
        cocoon.name
    ))
    .default(true)
    .run()
    .unwrap_or(false);

    if !confirm {
        out_warn!("Cancelled");
        return Ok(());
    }

    out_info!("Updating '{}'...", cocoon.name);
    let result = runtime.update(&cocoon.name)?;
    out_success!("{}", result);

    Ok(())
}

/// Handle check-update command interactively
fn handle_check_update_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to check for updates:")?;
    let runtime = manager.get_runtime(cocoon.runtime);

    let result = runtime.check_update(&cocoon.name)?;
    out_info!("{}", result);

    Ok(())
}

/// Handle remove command interactively
fn handle_remove_interactive(manager: &RuntimeManager) -> Result<(), String> {
    let cocoon = select_cocoon(manager, "Select cocoon to remove:")?;
    let runtime = manager.get_runtime(cocoon.runtime);

    let confirm = Confirm::new(format!("Remove cocoon '{}'?", cocoon.name))
        .default(false)
        .run()
        .unwrap_or(false);

    if !confirm {
        out_warn!("Cancelled");
        return Ok(());
    }

    let force = matches!(cocoon.status, CocoonStatus::Running);
    if force {
        out_warn!("Cocoon is running, will force stop first...");
    }

    out_info!("Removing '{}'...", cocoon.name);
    let result = runtime.remove(&cocoon.name, force)?;
    out_success!("{}", result);

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

    let options: Vec<SelectOption<RuntimeType>> = runtimes
        .into_iter()
        .map(|rt| {
            let desc = match rt {
                RuntimeType::Docker => "Container runtime",
                RuntimeType::Machine => "Native service",
            };
            SelectOption::new(rt.to_string(), rt).with_description(desc)
        })
        .collect();

    let runtime_type = Select::new("Select runtime:")
        .options(options)
        .run()
        .ok_or_else(|| "Selection cancelled".to_string())?;

    match runtime_type {
        RuntimeType::Docker => create_docker_cocoon_interactive(),
        RuntimeType::Machine => create_machine_cocoon_interactive(),
    }
}

/// Create a Docker cocoon interactively
fn create_docker_cocoon_interactive() -> Result<(), String> {
    let name = Input::new("Container name:")
        .default("cocoon-worker")
        .run()
        .ok_or_else(|| "Cancelled".to_string())?;

    let signaling_url = Input::new("Signaling server URL:")
        .default("ws://localhost:8080/ws")
        .run()
        .ok_or_else(|| "Cancelled".to_string())?;

    let setup_token = Input::new("Setup token (optional):")
        .run()
        .ok_or_else(|| "Cancelled".to_string())?;

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

    docker_cmd.arg("docker-registry.the-ihor.com/cocoon:latest");

    out_info!("Creating Docker cocoon '{}'...", name);

    match docker_cmd.output() {
        Ok(output) if output.status.success() => {
            let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            out_success!("Container created: {}", container_id);
            out_info!("View logs: adi cocoon logs {}", name);
            out_info!("Stop: adi cocoon stop {}", name);
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
    let signaling_url = Input::new("Signaling server URL:")
        .default("ws://localhost:8080/ws")
        .run()
        .ok_or_else(|| "Cancelled".to_string())?;

    // Set env var for install function
    std::env::set_var("SIGNALING_SERVER_URL", &signaling_url);

    let setup_token = Input::new("Setup token (optional):")
        .run()
        .ok_or_else(|| "Cancelled".to_string())?;

    if !setup_token.is_empty() {
        std::env::set_var("COCOON_SETUP_TOKEN", &setup_token);
    }

    out_info!("Starting cocoon via ADI daemon...");
    crate::ensure_daemon_running()?;
    out_success!("Cocoon service registered with ADI daemon");

    Ok(())
}

/// Print help text
fn print_help() {
    Section::new("Cocoon Interactive Mode - Help").print();

    out_info!("{}", theme::bold("Commands:"));
    Columns::new()
        .header(["Command", "Description"])
        .row(["list", "List all cocoons across all runtimes"])
        .row(["status", "Show detailed status for a specific cocoon"])
        .row(["start", "Start a stopped cocoon"])
        .row(["stop", "Stop a running cocoon"])
        .row(["restart", "Restart a cocoon"])
        .row(["logs", "View logs for a cocoon"])
        .row(["update", "Update cocoon to latest version"])
        .row(["check", "Check for available updates"])
        .row(["remove", "Remove a cocoon (stops if running)"])
        .row(["create", "Create a new cocoon (Docker or Machine)"])
        .row(["help", "Show this help"])
        .row(["exit", "Exit interactive mode"])
        .print();

    out_info!("{}", theme::bold("Runtimes:"));
    Columns::new()
        .header(["Runtime", "Description"])
        .row(["docker", "Docker containers — pulls latest image and recreates"])
        .row(["machine", "Native systemd/launchd — downloads binary and restarts"])
        .print();

    out_info!("{}", theme::bold("Tips:"));
    List::new()
        .item("Use arrow keys to navigate menus")
        .item("Press Enter to select")
        .item("Press Ctrl+C to cancel")
        .print();

    out_info!("{}", theme::bold("Non-interactive usage:"));
    List::new()
        .item("adi cocoon list")
        .item("adi cocoon start <name>")
        .item("adi cocoon stop <name>")
        .item("adi cocoon logs <name> [-f]")
        .item("adi cocoon update <name>")
        .item("adi cocoon check-update <name>")
        .print();
}
