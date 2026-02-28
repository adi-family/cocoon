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
mod setup;
pub mod silk;
pub mod webrtc;

pub use adi_router::{
    create_stream_channel, AdiHandleResult, AdiRouter, AdiService, AdiServiceError, StreamSender,
};
pub use core::run;
pub use runtime::{CocoonInfo, CocoonStatus, Runtime, RuntimeManager, RuntimeType};
pub use silk::{AnsiToHtml, SilkSession};
pub use webrtc::WebRtcManager;

#[cfg(feature = "tasks-core")]
pub use services::TasksService;

use lib_console_output::{out_error, out_info, out_success, theme, KeyValue, Renderable};
use lib_env_parse::{env_opt, env_vars};
use once_cell::sync::OnceCell;

static RUNTIME: OnceCell<tokio::runtime::Runtime> = OnceCell::new();

pub(crate) fn get_runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime")
    })
}

pub(crate) async fn ensure_daemon_running_async() -> std::result::Result<(), String> {
    start_cocoon_daemon(&[]).await
}

/// Start the cocoon daemon service, optionally injecting extra env vars.
///
/// If the service is already running without extra env, it's left alone.
/// If extra env is provided, an existing service is restarted with the new vars.
pub(crate) async fn start_cocoon_daemon(
    extra_env: &[(&str, &str)],
) -> std::result::Result<(), String> {
    let client = lib_daemon_client::DaemonClient::new();

    client
        .ensure_running()
        .await
        .map_err(|e| format!("Failed to start adi daemon: {}", e))?;

    let services = client
        .list_services()
        .await
        .map_err(|e| format!("Failed to list services: {}", e))?;

    let running = services
        .iter()
        .any(|s| s.name == "adi.cocoon" && s.state.is_running());

    // If extra env is provided and service is already running, restart it.
    if running && !extra_env.is_empty() {
        out_info!("Restarting cocoon service with new configuration...");
        let _ = client.stop_service("adi.cocoon", false).await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    } else if running {
        return Ok(());
    }

    // Build config with extra env vars
    let config = if extra_env.is_empty() {
        None
    } else {
        let exe = std::env::current_exe()
            .map_err(|e| format!("Failed to get exe path: {}", e))?;
        let mut cfg = lib_daemon_client::ServiceConfig::new(exe.display().to_string())
            .args(["daemon", "run-service", "adi.cocoon"])
            .env("RUST_LOG", "trace");
        for &(key, value) in extra_env {
            cfg = cfg.env(key, value);
        }
        Some(cfg)
    };

    out_info!("Starting cocoon service...");
    client
        .start_service("adi.cocoon", config)
        .await
        .map_err(|e| format!("Failed to start cocoon service: {}", e))?;

    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        if let Ok(svcs) = client.list_services().await {
            if svcs
                .iter()
                .any(|s| s.name == "adi.cocoon" && s.state.is_running())
            {
                break;
            }
        }
    }
    out_success!("Cocoon service started");

    Ok(())
}

/// Sync wrapper for callers not already inside a tokio runtime.
pub(crate) fn ensure_daemon_running() -> std::result::Result<(), String> {
    get_runtime().block_on(ensure_daemon_running_async())
}

env_vars! {
    SignalingServerUrl => "SIGNALING_SERVER_URL",
    Home => "HOME",
    CocoonSetupToken => "COCOON_SETUP_TOKEN",
    CocoonSecret => "COCOON_SECRET",
}

use lib_plugin_prelude::*;

// === CLI Args ===

#[derive(CliArgs)]
pub struct NameArg {
    #[arg(position = 0)]
    pub name: Option<String>,
}

#[derive(CliArgs)]
pub struct LogsArgs {
    #[arg(position = 0)]
    pub name: Option<String>,

    #[arg(long = "f")]
    pub follow: bool,

    #[arg(long)]
    pub tail: Option<u32>,
}

#[derive(CliArgs)]
pub struct RmArgs {
    #[arg(position = 0)]
    pub name: Option<String>,

    #[arg(long)]
    pub force: bool,
}

#[derive(CliArgs)]
pub struct CreateArgs {
    #[arg(long)]
    pub runtime: Option<String>,

    #[arg(long)]
    pub name: Option<String>,

    #[arg(long)]
    pub url: Option<String>,

    #[arg(long)]
    pub token: Option<String>,

    #[arg(long)]
    pub secret: Option<String>,

    #[arg(long)]
    pub start: bool,
}

#[derive(CliArgs)]
pub struct SetupArgs {
    #[arg(long)]
    pub port: Option<u16>,
    #[arg(long)]
    pub url: Option<String>,
}

#[derive(CliArgs)]
pub struct CheckUpdateArgs {
    #[arg(position = 0)]
    pub name: Option<String>,
}

#[derive(CliArgs)]
pub struct UpdateArgs {
    #[arg(position = 0)]
    pub name: Option<String>,

    #[arg(long)]
    pub all: bool,
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
) -> std::result::Result<String, String> {
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

    out_info!("Creating Docker cocoon '{}'...", name);

    match docker_cmd.output() {
        Ok(output) if output.status.success() => {
            let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            out_success!("Container created: {}", container_id);
            out_info!("Manage cocoon:");
            out_info!("  adi cocoon status {}", name);
            out_info!("  adi cocoon logs {} -f", name);
            out_info!("  adi cocoon stop {}", name);
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
    setup [--port PORT] Start pairing server for browser setup (default: 14730)
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
        PluginMetadata::new("adi.cocoon", "Cocoon", env!("CARGO_PKG_VERSION"))
            .with_type(PluginType::Core)
            .with_author("ADI Team")
            .with_description("Remote containerized worker with PTY support")
    }

    async fn init(&mut self, _ctx: &PluginContext) -> Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
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
            Self::__sdk_cmd_meta_list(),
            Self::__sdk_cmd_meta_status(),
            Self::__sdk_cmd_meta_start_cocoon(),
            Self::__sdk_cmd_meta_stop(),
            Self::__sdk_cmd_meta_restart(),
            Self::__sdk_cmd_meta_logs(),
            Self::__sdk_cmd_meta_rm(),
            Self::__sdk_cmd_meta_create(),
            Self::__sdk_cmd_meta_run_native(),
            Self::__sdk_cmd_meta_setup_pairing(),
            Self::__sdk_cmd_meta_check_update(),
            Self::__sdk_cmd_meta_update(),
            Self::__sdk_cmd_meta_version(),
        ]
    }

    async fn run_command(&self, ctx: &CliContext) -> Result<CliResult> {
        match ctx.subcommand.as_deref() {
            Some("list") | Some("ls") | Some("ps") => self.__sdk_cmd_handler_list(ctx).await,
            Some("status") => self.__sdk_cmd_handler_status(ctx).await,
            Some("start") => self.__sdk_cmd_handler_start_cocoon(ctx).await,
            Some("stop") => self.__sdk_cmd_handler_stop(ctx).await,
            Some("restart") => self.__sdk_cmd_handler_restart(ctx).await,
            Some("logs") => self.__sdk_cmd_handler_logs(ctx).await,
            Some("rm") | Some("remove") => self.__sdk_cmd_handler_rm(ctx).await,
            Some("create") | Some("new") => self.__sdk_cmd_handler_create(ctx).await,
            Some("run") => self.__sdk_cmd_handler_run_native(ctx).await,
            Some("setup") => self.__sdk_cmd_handler_setup_pairing(ctx).await,
            Some("check-update") | Some("check") => self.__sdk_cmd_handler_check_update(ctx).await,
            Some("update") | Some("upgrade") | Some("self-update") => {
                self.__sdk_cmd_handler_update(ctx).await
            }
            Some("version") | Some("-v") | Some("-V") | Some("--version") => {
                self.__sdk_cmd_handler_version(ctx).await
            }
            Some("help") | Some("-h") | Some("--help") => {
                Ok(CliResult::success(get_help_text().to_string()))
            }
            Some("") | Some("interactive") | Some("i") | None => {
                let manager = RuntimeManager::new();
                if let Err(e) = interactive::run_interactive(&manager) {
                    return Ok(CliResult::error(e));
                }
                Ok(CliResult::success("Interactive mode exited".to_string()))
            }
            Some(cmd) => Ok(CliResult::error(format!(
                "Unknown command: {}. Run 'adi cocoon help' for usage information.",
                cmd
            ))),
        }
    }
}

// === Command Handlers ===

impl CocoonPlugin {
    #[command(name = "list", description = "List all cocoons")]
    async fn list(&self) -> CmdResult {
        let manager = RuntimeManager::new();
        interactive::handle_list(&manager).map_err(|e| e)?;
        Ok("Listed cocoons".to_string())
    }

    #[command(name = "status", description = "Show cocoon status")]
    async fn status(&self, args: NameArg) -> CmdResult {
        let manager = RuntimeManager::new();
        if let Some(name) = args.name {
            match manager.find_cocoon(&name) {
                Some((_, runtime_type)) => {
                    let runtime = manager.get_runtime(runtime_type);
                    match runtime.status(&name) {
                        Ok(info) => {
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
                            Ok(format!("Status: {}", info.status))
                        }
                        Err(e) => Err(e),
                    }
                }
                None => Err(format!("Cocoon '{}' not found", name)),
            }
        } else {
            interactive::run_interactive(&manager).map_err(|e| e)?;
            Ok("Done".to_string())
        }
    }

    #[command(name = "start", description = "Start a stopped cocoon")]
    async fn start_cocoon(&self, args: NameArg) -> CmdResult {
        let manager = RuntimeManager::new();
        if let Some(name) = args.name {
            match manager.find_cocoon(&name) {
                Some((_, runtime_type)) => {
                    let runtime = manager.get_runtime(runtime_type);
                    out_info!("Starting '{}'...", name);
                    runtime.start(&name)
                }
                None => Err(format!(
                    "Cocoon '{}' not found. Use 'adi cocoon list' to see available cocoons.",
                    name
                )),
            }
        } else {
            interactive::run_interactive(&manager).map_err(|e| e)?;
            Ok("Done".to_string())
        }
    }

    #[command(name = "stop", description = "Stop a running cocoon")]
    async fn stop(&self, args: NameArg) -> CmdResult {
        let manager = RuntimeManager::new();
        if let Some(name) = args.name {
            match manager.find_cocoon(&name) {
                Some((_, runtime_type)) => {
                    let runtime = manager.get_runtime(runtime_type);
                    out_info!("Stopping '{}'...", name);
                    runtime.stop(&name)
                }
                None => Err(format!("Cocoon '{}' not found", name)),
            }
        } else {
            interactive::run_interactive(&manager).map_err(|e| e)?;
            Ok("Done".to_string())
        }
    }

    #[command(name = "restart", description = "Restart a cocoon")]
    async fn restart(&self, args: NameArg) -> CmdResult {
        let manager = RuntimeManager::new();
        if let Some(name) = args.name {
            match manager.find_cocoon(&name) {
                Some((_, runtime_type)) => {
                    let runtime = manager.get_runtime(runtime_type);
                    out_info!("Restarting '{}'...", name);
                    runtime.restart(&name)
                }
                None => Err(format!("Cocoon '{}' not found", name)),
            }
        } else {
            interactive::run_interactive(&manager).map_err(|e| e)?;
            Ok("Done".to_string())
        }
    }

    #[command(name = "logs", description = "View cocoon logs")]
    async fn logs(&self, args: LogsArgs) -> CmdResult {
        let manager = RuntimeManager::new();
        if let Some(name) = args.name {
            match manager.find_cocoon(&name) {
                Some((_, runtime_type)) => {
                    let runtime = manager.get_runtime(runtime_type);
                    runtime.logs(&name, args.follow, args.tail).map_err(|e| e)?;
                    Ok("Logs displayed".to_string())
                }
                None => Err(format!("Cocoon '{}' not found", name)),
            }
        } else {
            interactive::run_interactive(&manager).map_err(|e| e)?;
            Ok("Done".to_string())
        }
    }

    #[command(name = "rm", description = "Remove a cocoon")]
    async fn rm(&self, args: RmArgs) -> CmdResult {
        let manager = RuntimeManager::new();
        if let Some(name) = args.name {
            match manager.find_cocoon(&name) {
                Some((_, runtime_type)) => {
                    let runtime = manager.get_runtime(runtime_type);
                    out_info!("Removing '{}'...", name);
                    runtime.remove(&name, args.force)
                }
                None => Err(format!("Cocoon '{}' not found", name)),
            }
        } else {
            interactive::run_interactive(&manager).map_err(|e| e)?;
            Ok("Done".to_string())
        }
    }

    #[command(name = "create", description = "Create a new cocoon")]
    async fn create(&self, args: CreateArgs) -> CmdResult {
        let manager = RuntimeManager::new();
        if let Some(runtime_str) = args.runtime {
            let runtime_type = RuntimeType::from_str(&runtime_str).ok_or_else(|| {
                format!(
                    "Invalid runtime '{}'. Use 'docker' or 'machine'.",
                    runtime_str
                )
            })?;
            match runtime_type {
                RuntimeType::Docker => {
                    let name = args.name.unwrap_or_else(generate_container_name);
                    let signaling_url = args
                        .url
                        .or_else(|| env_opt(EnvVar::SignalingServerUrl.as_str()))
                        .unwrap_or_else(|| "ws://localhost:8080/ws".to_string());
                    let setup_token = args
                        .token
                        .or_else(|| env_opt(EnvVar::CocoonSetupToken.as_str()));
                    let cocoon_secret = args
                        .secret
                        .or_else(|| env_opt(EnvVar::CocoonSecret.as_str()));
                    create_docker_cocoon(
                        &name,
                        &signaling_url,
                        setup_token.as_deref(),
                        cocoon_secret.as_deref(),
                    )
                }
                RuntimeType::Machine => {
                    ensure_daemon_running()?;
                    out_success!("Cocoon service registered with ADI daemon");
                    Ok("Machine cocoon created".to_string())
                }
            }
        } else {
            interactive::run_interactive(&manager).map_err(|e| e)?;
            Ok("Done".to_string())
        }
    }

    #[command(name = "run", description = "Run cocoon natively in foreground")]
    async fn run_native(&self) -> CmdResult {
        run_with_runtime(async {
            if let Err(e) = core::run().await {
                out_error!("Cocoon error: {}", e);
            }
            Ok("Cocoon stopped".to_string())
        })
    }

    #[command(name = "setup", description = "Start pairing server for browser setup")]
    async fn setup_pairing(&self, args: SetupArgs) -> CmdResult {
        let port = args.port.unwrap_or(14730);
        let url = args.url;
        run_with_runtime(async move {
            setup::run_setup(port, url).await
        })
    }

    #[command(name = "check-update", description = "Check for available updates")]
    async fn check_update(&self, args: CheckUpdateArgs) -> CmdResult {
        let manager = RuntimeManager::new();
        if let Some(name) = args.name {
            match manager.find_cocoon(&name) {
                Some((_, runtime_type)) => {
                    let runtime = manager.get_runtime(runtime_type);
                    match runtime.check_update(&name) {
                        Ok(msg) => {
                            out_info!("{}", msg);
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
            match manager.list_all() {
                Ok(cocoons) if cocoons.is_empty() => {
                    out_info!("No cocoons found. Create one with: adi cocoon create");
                    Ok("No cocoons found".to_string())
                }
                Ok(cocoons) => {
                    let mut results = Vec::new();
                    for info in cocoons {
                        let runtime = manager.get_runtime(info.runtime);
                        out_info!("{} ({})", info.name, info.runtime);
                        match runtime.check_update(&info.name) {
                            Ok(msg) => {
                                out_info!("{}", msg);
                                results.push(format!("{}: OK", info.name));
                            }
                            Err(e) => {
                                out_error!("Error: {}", e);
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

    #[command(name = "update", description = "Update cocoon to latest version")]
    async fn update(&self, args: UpdateArgs) -> CmdResult {
        let manager = RuntimeManager::new();
        if let Some(name) = args.name {
            match manager.find_cocoon(&name) {
                Some((_, runtime_type)) => {
                    let runtime = manager.get_runtime(runtime_type);
                    match runtime.update(&name) {
                        Ok(msg) => {
                            out_info!("{}", msg);
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
        } else if args.all {
            match manager.list_all() {
                Ok(cocoons) if cocoons.is_empty() => {
                    out_info!("No cocoons found. Create one with: adi cocoon create");
                    Ok("No cocoons found".to_string())
                }
                Ok(cocoons) => {
                    let mut results = Vec::new();
                    for info in cocoons {
                        let runtime = manager.get_runtime(info.runtime);
                        out_info!("Updating {} ({})...", info.name, info.runtime);
                        match runtime.update(&info.name) {
                            Ok(msg) => {
                                out_info!("{}", msg);
                                results.push(format!("{}: Updated", info.name));
                            }
                            Err(e) => {
                                out_error!("Error: {}", e);
                                results.push(format!("{}: Failed", info.name));
                            }
                        }
                    }
                    out_info!("Update Summary:");
                    for r in &results {
                        out_info!("  {}", r);
                    }
                    Ok(results.join(", "))
                }
                Err(e) => Err(e),
            }
        } else {
            interactive::run_interactive(&manager).map_err(|e| e)?;
            Ok("Done".to_string())
        }
    }

    #[command(name = "version", description = "Show current version")]
    async fn version(&self) -> CmdResult {
        let version = env!("CARGO_PKG_VERSION");
        out_info!("cocoon {}", version);
        Ok(format!("cocoon {}", version))
    }
}

#[daemon_service]
impl CocoonPlugin {
    async fn start(&self, _ctx: DaemonContext) -> Result<()> {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "cocoon=info".into()),
            )
            .with_ansi(false)
            .try_init();

        let (tx, rx) = tokio::sync::oneshot::channel::<std::result::Result<(), String>>();

        std::thread::Builder::new()
            .name("cocoon-daemon".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = tx.send(Err(format!("Failed to build Tokio runtime: {}", e)));
                        return;
                    }
                };

                let result = rt.block_on(async {
                    core::run().await.map_err(|e| e.to_string())
                });

                let _ = tx.send(result);
            })
            .map_err(|e| anyhow::anyhow!("Failed to spawn daemon thread: {}", e))?;

        rx.await
            .map_err(|_| anyhow::anyhow!("Daemon thread terminated unexpectedly"))?
            .map_err(|e| anyhow::anyhow!("{}", e).into())
    }
}

/// Run an async block on a dedicated Tokio runtime.
///
/// Plugin cdylibs have their own copy of Tokio that doesn't share the host's
/// reactor, so any async I/O (TCP listeners, spawned tasks, etc.) must run on
/// a runtime owned by the plugin.
fn run_with_runtime<F: std::future::Future<Output = CmdResult> + Send + 'static>(
    fut: F,
) -> CmdResult {
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .map_err(|e| format!("Failed to create runtime: {e}"))?
            .block_on(fut)
    })
    .join()
    .map_err(|_| "Async task panicked".to_string())?
}

/// ABI version for host compatibility check.
#[no_mangle]
pub extern "C" fn plugin_abi_version() -> u32 {
    3
}

#[no_mangle]
pub fn plugin_create() -> Box<dyn Plugin> {
    Box::new(CocoonPlugin::new())
}

#[no_mangle]
pub fn plugin_create_cli() -> Box<dyn CliCommands> {
    Box::new(CocoonPlugin::new())
}
