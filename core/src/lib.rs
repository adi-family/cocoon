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

pub use interactive::{handle_list, run_interactive};
pub use setup::run_setup;

use lib_console_output::{out_info, out_success};
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

    if running && !extra_env.is_empty() {
        out_info!("Restarting cocoon service with new configuration...");
        let _ = client.stop_service("adi.cocoon", false).await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    } else if running {
        return Ok(());
    }

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

pub(crate) fn ensure_daemon_running() -> std::result::Result<(), String> {
    get_runtime().block_on(ensure_daemon_running_async())
}

env_vars! {
    SignalingServerUrl => "SIGNALING_SERVER_URL",
    Home => "HOME",
    CocoonSetupToken => "COCOON_SETUP_TOKEN",
    CocoonSecret => "COCOON_SECRET",
}
