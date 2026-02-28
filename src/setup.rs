//! Browser pairing server for cocoon setup.
//!
//! Starts a local HTTP server that the browser connects to with a setup token,
//! then installs and starts the cocoon as a machine runtime service.

use lib_console_output::{out_info, out_success};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Run the pairing setup flow: HTTP server → browser connect → install + start cocoon.
pub async fn run_setup(port: u16, cli_url: Option<String>) -> Result<String, String> {
    let hostname = get_machine_name();
    let (connect_tx, mut connect_rx) = tokio::sync::mpsc::channel::<ConnectRequest>(1);

    // If --url provided, store it so /connect can use it as override.
    let signaling_override = cli_url.map(|u| Arc::new(u));

    let state = Arc::new(SetupServerState {
        connected: RwLock::new(false),
        hostname: hostname.clone(),
        connect_tx,
        signaling_override: signaling_override.clone(),
    });

    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);

    let app = axum::Router::new()
        .route("/health", axum::routing::get(health_handler))
        .route("/connect", axum::routing::post(connect_handler))
        .layer(cors)
        .with_state(state);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));

    out_info!("Starting ADI local server...");
    out_info!("  Name: {}", hostname);
    out_info!("  URL:  http://localhost:{}", port);
    if let Some(ref url) = signaling_override {
        out_info!("  Signaling: {}", url);
    }
    out_info!("Waiting for browser connection... (Ctrl+C to stop)");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("Failed to bind port {}: {}", port, e))?;

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await
    });

    let result = if let Some(req) = connect_rx.recv().await {
        let signaling_url = signaling_override
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&req.signaling_url);

        out_info!("Browser connected! Setting up cocoon...");
        out_info!("  Signaling: {}", signaling_url);
        if !req.token.is_empty() {
            let masked = if req.token.len() > 8 {
                format!("{}...{}", &req.token[..4], &req.token[req.token.len()-4..])
            } else {
                "****".to_string()
            };
            out_info!("  Token:     {}", masked);
        }

        let mut env: Vec<(&str, &str)> = vec![
            ("SIGNALING_SERVER_URL", signaling_url),
        ];
        if !req.token.is_empty() {
            env.push(("COCOON_SETUP_TOKEN", &req.token));
        }

        // Start cocoon via ADI daemon with the signaling URL injected
        match crate::start_cocoon_daemon(&env).await {
            Ok(()) => {
                out_success!("Cocoon service registered with ADI daemon");
                Ok("Cocoon installed and running as a background service".to_string())
            }
            Err(e) => Err(format!("Failed to start cocoon service: {}", e)),
        }
    } else {
        Ok("Setup cancelled".to_string())
    };

    server.abort();
    result
}

/// Shared state for the setup server.
struct SetupServerState {
    connected: RwLock<bool>,
    hostname: String,
    connect_tx: tokio::sync::mpsc::Sender<ConnectRequest>,
    signaling_override: Option<Arc<String>>,
}

/// Request body for the /connect endpoint.
#[derive(serde::Deserialize)]
struct ConnectRequest {
    token: String,
    #[serde(default = "default_signaling_url")]
    signaling_url: String,
}

fn default_signaling_url() -> String {
    crate::env_opt(crate::EnvVar::SignalingServerUrl.as_str())
        .unwrap_or_else(|| "ws://localhost:8080/ws".to_string())
}

/// Health endpoint for browser polling.
async fn health_handler(
    axum::extract::State(state): axum::extract::State<Arc<SetupServerState>>,
) -> axum::Json<serde_json::Value> {
    let connected = *state.connected.read().await;

    let mut body = serde_json::json!({
        "status": "ok",
        "name": state.hostname,
        "version": env!("CARGO_PKG_VERSION"),
        "connected": connected
    });
    if let Some(ref url) = state.signaling_override {
        body["signaling_url"] = serde_json::Value::String(url.as_ref().clone());
    }
    axum::Json(body)
}

/// Connect endpoint — browser sends token to register with platform.
async fn connect_handler(
    axum::extract::State(state): axum::extract::State<Arc<SetupServerState>>,
    axum::Json(req): axum::Json<ConnectRequest>,
) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    if *state.connected.read().await {
        return (
            axum::http::StatusCode::OK,
            axum::Json(serde_json::json!({
                "status": "already_connected",
                "name": state.hostname
            })),
        );
    }

    if let Err(e) = state.connect_tx.send(req).await {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({
                "status": "error",
                "message": format!("Failed to process request: {}", e)
            })),
        );
    }

    *state.connected.write().await = true;

    (
        axum::http::StatusCode::OK,
        axum::Json(serde_json::json!({
            "status": "connecting",
            "name": state.hostname
        })),
    )
}

/// Get a friendly machine name for display.
fn get_machine_name() -> String {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        if let Ok(output) = Command::new("scutil").args(["--get", "ComputerName"]).output() {
            if output.status.success() {
                if let Ok(name) = String::from_utf8(output.stdout) {
                    let name = name.trim();
                    if !name.is_empty() {
                        return name.to_string();
                    }
                }
            }
        }
    }

    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "Local Machine".to_string())
}
