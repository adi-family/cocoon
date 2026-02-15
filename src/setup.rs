//! Browser pairing server for cocoon setup.
//!
//! Starts a local HTTP server that the browser connects to with a setup token,
//! then installs and starts the cocoon as a machine runtime service.

use std::sync::Arc;
use tokio::sync::RwLock;

/// Run the pairing setup flow: HTTP server → browser connect → install + start cocoon.
pub async fn run_setup(port: u16) -> Result<String, String> {
    let hostname = get_machine_name();
    let (connect_tx, mut connect_rx) = tokio::sync::mpsc::channel::<ConnectRequest>(1);

    let state = Arc::new(SetupServerState {
        connected: RwLock::new(false),
        hostname: hostname.clone(),
        connect_tx,
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

    println!("\nStarting ADI local server...\n");
    println!("  Name: {}", hostname);
    println!("  URL:  http://localhost:{}", port);
    println!("\nWaiting for browser connection... (Ctrl+C to stop)\n");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("Failed to bind port {}: {}", port, e))?;

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await
    });

    let result = if let Some(req) = connect_rx.recv().await {
        println!("Browser connected! Setting up cocoon...\n");

        std::env::set_var("SIGNALING_SERVER_URL", &req.signaling_url);
        std::env::set_var("COCOON_SETUP_TOKEN", &req.token);
        std::env::set_var("COCOON_NAME", &hostname);

        // Install as machine runtime service
        let manager = crate::RuntimeManager::new();
        let runtime = manager.get_runtime(crate::RuntimeType::Machine);
        let _ = runtime.stop("cocoon");

        match crate::service_install() {
            Ok(msg) => {
                println!("{}", msg);
                match crate::service_start() {
                    Ok(start_msg) => println!("{}", start_msg),
                    Err(e) => println!("Warning: Failed to start service: {}", e),
                }
                Ok("Cocoon installed and running as a background service".to_string())
            }
            Err(e) => Err(format!("Failed to install cocoon service: {}", e)),
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

    axum::Json(serde_json::json!({
        "status": "ok",
        "name": state.hostname,
        "version": env!("CARGO_PKG_VERSION"),
        "connected": connected
    }))
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
