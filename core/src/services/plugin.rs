//! Plugin handler — processes typed `plugin_*` protocol messages.
//!
//! Installs ADI plugins on the cocoon by shelling out to the `adi` CLI.

use crate::protocol::messages::CocoonMessage;
use tokio::process::Command;

/// Resolve the path to the `adi` binary.
fn adi_binary() -> String {
    std::env::var("ADI_BIN").unwrap_or_else(|_| "adi".to_string())
}

/// Handle a `PluginInstallPlugin` message and return the response.
pub async fn handle_install(
    request_id: String,
    plugin_id: String,
    registry: Option<String>,
    version: Option<String>,
) -> CocoonMessage {
    tracing::info!("📦 Installing plugin: {}", plugin_id);

    let mut cmd = Command::new(adi_binary());
    cmd.arg("plugin").arg("install").arg(&plugin_id);

    if let Some(ver) = &version {
        cmd.arg("--version").arg(ver);
    }
    if let Some(reg) = &registry {
        cmd.env("ADI_REGISTRY_URL", reg);
    }

    match cmd.output().await {
        Ok(output) => {
            let success = output.status.success();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            if success {
                tracing::info!("✅ Plugin installed: {}", plugin_id);
            } else {
                tracing::warn!("❌ Plugin install failed: {}", plugin_id);
            }

            CocoonMessage::PluginInstallPluginResponse {
                request_id,
                success,
                plugin_id,
                stdout,
                stderr,
            }
        }
        Err(e) => CocoonMessage::PluginInstallError {
            request_id,
            plugin_id,
            code: "spawn_failed".to_string(),
            message: format!("Failed to spawn adi CLI: {e}"),
        },
    }
}
