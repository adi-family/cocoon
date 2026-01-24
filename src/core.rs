use crate::silk::{AnsiToHtml, SilkSession};
use futures::{SinkExt, StreamExt};
use lib_tarminal_sync::{QueryType, SignalingMessage, SilkResponse, SilkStream};
use portable_pty::{CommandBuilder, PtySize};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

const OUTPUT_DIR: &str = "/cocoon/output";
const RESPONSE_PATH: &str = "/cocoon/output/response.json";
const SECRET_PATH: &str = "/cocoon/.secret";
const DEVICE_ID_PATH: &str = "/cocoon/.device_id";

// Secret security requirements
const MIN_SECRET_LENGTH: usize = 32;
const GENERATED_SECRET_LENGTH: usize = 48; // 288 bits of entropy

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CommandRequest {
    /// Execute a simple command (non-interactive)
    Execute {
        command: String,
        input: Option<String>,
    },

    /// Attach a PTY session (interactive terminal)
    AttachPty {
        command: String,
        cols: u16,
        rows: u16,
        #[serde(default)]
        env: HashMap<String, String>,
    },

    /// Send input to PTY session
    PtyInput { session_id: Uuid, data: String },

    /// Resize PTY terminal (remote controls size)
    PtyResize {
        session_id: Uuid,
        cols: u16,
        rows: u16,
    },

    /// Close PTY session
    PtyClose { session_id: Uuid },

    /// Proxy HTTP request to local service
    ProxyHttp {
        request_id: String,
        service_name: String,
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Option<String>,
    },

    /// Query local data (for aggregation)
    QueryLocal {
        query_id: String,
        query_type: QueryType,
        params: JsonValue,
    },

    // ========== Silk Terminal Commands ==========
    /// Create a new Silk session
    SilkCreateSession {
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default)]
        env: HashMap<String, String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        shell: Option<String>,
    },

    /// Execute command in Silk session
    SilkExecute {
        session_id: Uuid,
        command: String,
        command_id: Uuid,
    },

    /// Send input to running Silk command (for interactive mode)
    SilkInput {
        session_id: Uuid,
        command_id: Uuid,
        data: String,
    },

    /// Resize Silk interactive terminal
    SilkResize {
        session_id: Uuid,
        command_id: Uuid,
        cols: u16,
        rows: u16,
    },

    /// Close Silk session
    SilkCloseSession { session_id: Uuid },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CommandResponse {
    /// Result of Execute command
    ExecuteResult {
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ErrorInfo>,
        #[serde(default)]
        files: Vec<OutputFile>,
    },

    /// PTY session created successfully
    PtyCreated { session_id: Uuid },

    /// Output from PTY session (synced in real-time)
    PtyOutput { session_id: Uuid, data: String },

    /// PTY session exited
    PtyExited { session_id: Uuid, exit_code: i32 },

    /// HTTP proxy result
    ProxyResult {
        request_id: String,
        status_code: u16,
        headers: HashMap<String, String>,
        body: Option<String>,
    },

    /// Query result (for aggregation)
    QueryResult {
        query_id: String,
        data: JsonValue,
        is_final: bool,
    },

    /// Error response
    Error { code: String, message: String },

    /// Silk terminal response (wraps SilkResponse)
    #[serde(untagged)]
    SilkResponse(SilkResponse),
}

#[derive(Debug, Serialize)]
struct ErrorInfo {
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
}

#[derive(Debug, Serialize)]
struct OutputFile {
    path: String,
    content: String,
    binary: bool,
}

struct PtySession {
    #[allow(dead_code)]
    id: Uuid,
    pair: portable_pty::PtyPair,
    child: Box<dyn portable_pty::Child + Send>,
    writer: Box<dyn std::io::Write + Send>,
}

type SharedWriter = Arc<
    Mutex<
        futures::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
    >,
>;

async fn collect_output_files(dir: &str) -> Vec<OutputFile> {
    let mut files = Vec::new();
    let output_path = Path::new(dir);

    if !output_path.exists() {
        return files;
    }

    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().to_string_lossy() != RESPONSE_PATH)
    {
        let path = entry.path();
        let rel_path = path
            .strip_prefix(dir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());

        match tokio::fs::read(path).await {
            Ok(content) => {
                let is_binary = content.contains(&0);
                let content_str = if is_binary {
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &content)
                } else {
                    String::from_utf8_lossy(&content).to_string()
                };

                files.push(OutputFile {
                    path: rel_path,
                    content: content_str,
                    binary: is_binary,
                });
            }
            Err(_) => continue,
        }
    }

    files
}

async fn execute_command(command: &str, input: Option<&str>) -> CommandResponse {
    let _ = tokio::fs::create_dir_all(OUTPUT_DIR).await;

    let mut child = match tokio::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            return CommandResponse::ExecuteResult {
                success: false,
                data: None,
                error: Some(ErrorInfo {
                    code: "spawn_failed".into(),
                    details: Some(e.to_string()),
                }),
                files: vec![],
            };
        }
    };

    if let Some(input_str) = input {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input_str.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
    }

    let output = match child.wait_with_output().await {
        Ok(output) => output,
        Err(e) => {
            return CommandResponse::ExecuteResult {
                success: false,
                data: None,
                error: Some(ErrorInfo {
                    code: "execution_failed".into(),
                    details: Some(e.to_string()),
                }),
                files: vec![],
            };
        }
    };

    let files = collect_output_files(OUTPUT_DIR).await;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        CommandResponse::ExecuteResult {
            success: true,
            data: Some(serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": 0
            })),
            error: None,
            files,
        }
    } else {
        let exit_code = output.status.code().unwrap_or(-1);
        CommandResponse::ExecuteResult {
            success: false,
            data: Some(serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code
            })),
            error: Some(ErrorInfo {
                code: "command_failed".into(),
                details: Some(format!("exit code: {}", exit_code)),
            }),
            files,
        }
    }
}

async fn create_pty_session(
    command: &str,
    cols: u16,
    rows: u16,
    env: &HashMap<String, String>,
    writer: SharedWriter,
) -> Result<(Uuid, PtySession), String> {
    let session_id = Uuid::new_v4();
    let pty_system = portable_pty::native_pty_system();

    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("Failed to open PTY: {}", e))?;

    let mut cmd = CommandBuilder::new("/bin/sh");
    cmd.arg("-c");
    cmd.arg(command);

    // Set environment variables
    for (key, value) in env {
        cmd.env(key, value);
    }

    // Set TERM for proper terminal support
    cmd.env("TERM", "xterm-256color");

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Failed to spawn command: {}", e))?;

    // Spawn reader task to stream output in real-time
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("Failed to clone reader: {}", e))?;

    let session_id_clone = session_id;
    tokio::task::spawn_blocking(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buffer[..n]).to_string();
                    let response = CommandResponse::PtyOutput {
                        session_id: session_id_clone,
                        data,
                    };

                    let msg = SignalingMessage::SyncData {
                        payload: serde_json::to_value(&response).unwrap(),
                    };

                    // Send output to client (non-blocking)
                    let writer_clone = writer.clone();
                    tokio::spawn(async move {
                        let mut w = writer_clone.lock().await;
                        let _ = w
                            .send(Message::Text(serde_json::to_string(&msg).unwrap()))
                            .await;
                    });
                }
                Err(e) => {
                    tracing::warn!("PTY read error: {}", e);
                    break;
                }
            }
        }

        tracing::info!("PTY session {} reader task ended", session_id_clone);
    });

    // Take the writer once and store it
    let pty_writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("Failed to take PTY writer: {}", e))?;

    Ok((
        session_id,
        PtySession {
            id: session_id,
            pair,
            child,
            writer: pty_writer,
        },
    ))
}

/// Handle HTTP proxy request to local service
async fn handle_proxy_request(
    request_id: String,
    service_name: String,
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Option<String>,
    services: &HashMap<String, u16>,
) -> CommandResponse {
    // Look up service port in registry
    let port = match services.get(&service_name) {
        Some(port) => *port,
        None => {
            tracing::warn!("Service not found: {}", service_name);
            return CommandResponse::ProxyResult {
                request_id,
                status_code: 404,
                headers: HashMap::new(),
                body: Some(format!("Service not found: {}", service_name)),
            };
        }
    };

    // Build the target URL
    let url = format!("http://localhost:{}{}", port, path);
    tracing::debug!("Proxying {} {} to {}", method, path, url);

    // Create HTTP client
    let client = reqwest::Client::new();

    // Parse method
    let http_method = match method.to_uppercase().as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "DELETE" => reqwest::Method::DELETE,
        "PATCH" => reqwest::Method::PATCH,
        "HEAD" => reqwest::Method::HEAD,
        "OPTIONS" => reqwest::Method::OPTIONS,
        _ => {
            tracing::warn!("Unsupported HTTP method: {}", method);
            return CommandResponse::ProxyResult {
                request_id,
                status_code: 405,
                headers: HashMap::new(),
                body: Some(format!("Unsupported method: {}", method)),
            };
        }
    };

    // Build request
    let mut request_builder = client.request(http_method, &url);

    // Add headers
    for (key, value) in headers {
        request_builder = request_builder.header(&key, &value);
    }

    // Add body if present
    if let Some(body_str) = body {
        request_builder = request_builder.body(body_str);
    }

    // Send request with timeout
    match request_builder
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
    {
        Ok(response) => {
            let status_code = response.status().as_u16();
            let mut response_headers = HashMap::new();

            // Convert headers to HashMap
            for (key, value) in response.headers() {
                if let Ok(value_str) = value.to_str() {
                    response_headers.insert(key.to_string(), value_str.to_string());
                }
            }

            // Read response body
            let response_body = match response.text().await {
                Ok(text) => Some(text),
                Err(e) => {
                    tracing::warn!("Failed to read response body: {}", e);
                    None
                }
            };

            CommandResponse::ProxyResult {
                request_id,
                status_code,
                headers: response_headers,
                body: response_body,
            }
        }
        Err(e) => {
            tracing::error!("HTTP proxy request failed: {}", e);
            CommandResponse::ProxyResult {
                request_id,
                status_code: 502,
                headers: HashMap::new(),
                body: Some(format!("Proxy error: {}", e)),
            }
        }
    }
}

/// Handle local query for aggregation
async fn handle_query_local(
    query_id: String,
    query_type: QueryType,
    params: JsonValue,
) -> CommandResponse {
    match query_type {
        QueryType::ListTasks => {
            // For now, return empty task list
            // In production, this would query the local task store
            // Integration with lib-task-store will come later
            tracing::debug!("Listing local tasks with params: {:?}", params);

            CommandResponse::QueryResult {
                query_id,
                data: serde_json::json!({
                    "tasks": [],
                    "total": 0,
                    "source": "cocoon-local"
                }),
                is_final: true,
            }
        }
        QueryType::GetTaskStats => {
            tracing::debug!("Getting task stats");

            CommandResponse::QueryResult {
                query_id,
                data: serde_json::json!({
                    "pending": 0,
                    "running": 0,
                    "completed": 0,
                    "failed": 0,
                    "total": 0
                }),
                is_final: true,
            }
        }
        QueryType::SearchTasks => {
            let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
            tracing::debug!("Searching tasks for: {}", query);

            CommandResponse::QueryResult {
                query_id,
                data: serde_json::json!({
                    "tasks": [],
                    "query": query,
                    "total": 0
                }),
                is_final: true,
            }
        }
        QueryType::SearchKnowledgebase => {
            let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
            tracing::debug!("Searching knowledgebase for: {}", query);

            CommandResponse::QueryResult {
                query_id,
                data: serde_json::json!({
                    "results": [],
                    "query": query,
                    "total": 0
                }),
                is_final: true,
            }
        }
        QueryType::Custom { query_name } => {
            tracing::warn!("Custom query not implemented: {}", query_name);

            CommandResponse::QueryResult {
                query_id,
                data: serde_json::json!({
                    "error": format!("Custom query '{}' not implemented", query_name)
                }),
                is_final: true,
            }
        }
    }
}

/// Validate secret strength
fn validate_secret(secret: &str) -> Result<(), String> {
    if secret.len() < MIN_SECRET_LENGTH {
        return Err(format!(
            "Secret too short: {} characters (minimum: {})",
            secret.len(),
            MIN_SECRET_LENGTH
        ));
    }

    // Check for obvious weak patterns
    if secret.chars().all(|c| c.is_numeric()) {
        return Err("Secret must not be only numbers".to_string());
    }

    if secret.to_lowercase() == secret && secret.chars().all(|c| c.is_alphabetic()) {
        return Err("Secret must not be only lowercase letters".to_string());
    }

    if secret.chars().all(|c| c == secret.chars().next().unwrap()) {
        return Err("Secret must not be repetitive characters".to_string());
    }

    // Check for common weak patterns
    let lower = secret.to_lowercase();
    let weak_patterns = ["password", "secret", "admin", "12345", "qwerty", "test"];
    for pattern in &weak_patterns {
        if lower.contains(pattern) {
            return Err(format!("Secret contains weak pattern: {}", pattern));
        }
    }

    Ok(())
}

/// Generate cryptographically strong random secret
fn generate_strong_secret() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut rng = rand::thread_rng();

    (0..GENERATED_SECRET_LENGTH)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// Load device ID from file if it exists
async fn load_device_id() -> Option<String> {
    match tokio::fs::read_to_string(DEVICE_ID_PATH).await {
        Ok(device_id) => {
            let device_id = device_id.trim().to_string();
            if device_id.is_empty() {
                None
            } else {
                tracing::info!("📱 Loaded existing device ID from {}", DEVICE_ID_PATH);
                Some(device_id)
            }
        }
        Err(_) => None,
    }
}

/// Save device ID to file for future reconnections
async fn save_device_id(device_id: &str) {
    if let Err(e) = tokio::fs::write(DEVICE_ID_PATH, device_id).await {
        tracing::warn!("⚠️ Could not save device ID to {}: {}", DEVICE_ID_PATH, e);
        tracing::warn!("💡 Mount volume at /cocoon for persistent device ID");
    } else {
        tracing::info!(
            "💾 Saved device ID to {} for reconnection verification",
            DEVICE_ID_PATH
        );
    }
}

/// Send deregister message to signaling server
async fn send_deregister(writer: &SharedWriter, device_id: &str, reason: Option<&str>) {
    let deregister_msg = SignalingMessage::Deregister {
        device_id: device_id.to_string(),
        reason: reason.map(|r| r.to_string()),
    };

    let mut w = writer.lock().await;
    if let Err(e) = w
        .send(Message::Text(
            serde_json::to_string(&deregister_msg).unwrap(),
        ))
        .await
    {
        tracing::warn!("⚠️ Failed to send deregister message: {}", e);
    } else {
        tracing::info!("📤 Sent deregister message to server");
    }
}

/// Load or generate client secret for persistent device ID
/// Returns (secret, optional_device_id)
async fn get_or_create_secret() -> (String, Option<String>) {
    // Load device_id if it exists (for reconnection verification)
    let device_id = load_device_id().await;

    // Try environment variable first (for manual management)
    if let Ok(secret) = std::env::var("COCOON_SECRET") {
        tracing::info!("📋 Using secret from COCOON_SECRET environment variable");

        // Validate manual secret
        if let Err(e) = validate_secret(&secret) {
            tracing::error!("❌ Invalid secret from COCOON_SECRET: {}", e);
            tracing::error!("💡 Secret requirements:");
            tracing::error!("   - Minimum {} characters", MIN_SECRET_LENGTH);
            tracing::error!("   - Must be random and unpredictable");
            tracing::error!("   - Avoid common patterns, dictionary words");
            tracing::error!("   - Use: openssl rand -base64 36");
            std::process::exit(1);
        }

        return (secret, device_id);
    }

    // Try loading from file
    match tokio::fs::read_to_string(SECRET_PATH).await {
        Ok(secret) => {
            let secret = secret.trim().to_string();

            // Validate loaded secret
            if let Err(e) = validate_secret(&secret) {
                tracing::error!("❌ Invalid secret from {}: {}", SECRET_PATH, e);
                tracing::error!("💡 Deleting weak secret and generating new one");
                let _ = tokio::fs::remove_file(SECRET_PATH).await;
                // Also delete device_id since secret changed
                let _ = tokio::fs::remove_file(DEVICE_ID_PATH).await;
                // Fall through to generate new secret
            } else {
                tracing::info!("🔑 Loaded existing secret from {}", SECRET_PATH);
                return (secret, device_id);
            }
        }
        Err(_) => {
            // File doesn't exist, will generate new secret
        }
    }

    // Generate new cryptographically strong secret
    let secret = generate_strong_secret();
    tracing::info!(
        "🆕 Generated new cryptographically strong secret ({} chars, {} bits entropy)",
        GENERATED_SECRET_LENGTH,
        GENERATED_SECRET_LENGTH * 6
    );

    // Try to save it (may fail in read-only containers, that's ok)
    if let Err(e) = tokio::fs::write(SECRET_PATH, &secret).await {
        tracing::warn!(
            "⚠️ Could not save secret to {} (ephemeral session): {}",
            SECRET_PATH,
            e
        );
        tracing::warn!(
            "💡 Set COCOON_SECRET env var or mount volume at /cocoon for persistent sessions"
        );
    } else {
        tracing::info!("💾 Saved secret to {} for persistent sessions", SECRET_PATH);
    }

    // New secret means no device_id yet (first registration)
    (secret, None)
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("cocoon=info".parse().unwrap()),
        )
        .init();

    tracing::info!("🐛 Cocoon starting (v{})", env!("CARGO_PKG_VERSION"));

    // Get or create client secret and load device ID (for reconnection verification)
    let (secret, device_id) = get_or_create_secret().await;

    let signaling_url = std::env::var("SIGNALING_SERVER_URL")
        .unwrap_or_else(|_| "ws://localhost:8080/ws".to_string());

    tracing::info!("🔗 Connecting to signaling server: {}", signaling_url);

    let (ws_stream, _) = match connect_async(&signaling_url).await {
        Ok(conn) => conn,
        Err(e) => {
            tracing::error!("❌ Failed to connect to signaling server: {}", e);
            std::process::exit(1);
        }
    };

    let (write, mut read) = ws_stream.split();
    let writer = Arc::new(Mutex::new(write));

    // PTY sessions storage
    let pty_sessions: Arc<Mutex<HashMap<Uuid, PtySession>>> = Arc::new(Mutex::new(HashMap::new()));

    // Silk sessions storage
    let silk_sessions: Arc<Mutex<HashMap<Uuid, SilkSession>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // WebRTC signaling channel for sending messages back through WebSocket
    let (webrtc_tx, mut webrtc_rx) = tokio::sync::mpsc::unbounded_channel::<SignalingMessage>();

    // WebRTC session manager
    let webrtc_manager = Arc::new(crate::webrtc::WebRtcManager::new(webrtc_tx));

    // Spawn task to forward WebRTC signaling messages to WebSocket
    let writer_for_webrtc = writer.clone();
    tokio::spawn(async move {
        while let Some(msg) = webrtc_rx.recv().await {
            let mut w = writer_for_webrtc.lock().await;
            if let Err(e) = w
                .send(Message::Text(serde_json::to_string(&msg).unwrap_or_default()))
                .await
            {
                tracing::warn!("⚠️ Failed to send WebRTC signaling message: {}", e);
            }
        }
    });

    // Service registry - parse from COCOON_SERVICES env var
    // Format: "service1:port1,service2:port2"
    // Example: "flowmap-api:8092,postgres:5432"
    let mut services = HashMap::new();
    if let Ok(services_str) = std::env::var("COCOON_SERVICES") {
        for service_def in services_str.split(',') {
            let parts: Vec<&str> = service_def.trim().split(':').collect();
            if parts.len() == 2 {
                if let Ok(port) = parts[1].parse::<u16>() {
                    services.insert(parts[0].to_string(), port);
                    tracing::info!("📦 Registered service: {} → localhost:{}", parts[0], port);
                } else {
                    tracing::warn!("⚠️ Invalid port for service {}: {}", parts[0], parts[1]);
                }
            } else {
                tracing::warn!("⚠️ Invalid service definition: {}", service_def);
            }
        }
    }
    let services = Arc::new(services);

    // Check for setup token (one-command install flow)
    let setup_token = std::env::var("COCOON_SETUP_TOKEN").ok();
    let cocoon_name = std::env::var("COCOON_NAME").ok();

    // Register with signaling server
    // If setup token provided: use RegisterWithSetupToken (auto-claims ownership)
    // Otherwise: use Register (manual claiming required)
    let secret_for_claiming = secret.clone(); // Keep for displaying claiming instructions
    let cocoon_version = env!("CARGO_PKG_VERSION").to_string();
    let register_msg = if let Some(ref token) = setup_token {
        tracing::info!("🎫 Using setup token for auto-registration");
        SignalingMessage::RegisterWithSetupToken {
            secret,
            setup_token: token.clone(),
            name: cocoon_name.clone(),
            version: cocoon_version,
        }
    } else {
        SignalingMessage::Register {
            secret,
            device_id: device_id.clone(),
            version: cocoon_version,
        }
    };

    {
        let mut w = writer.lock().await;
        if let Err(e) = w
            .send(Message::Text(serde_json::to_string(&register_msg).unwrap()))
            .await
        {
            tracing::error!("❌ Failed to register: {}", e);
            std::process::exit(1);
        }
    }

    if setup_token.is_some() {
        tracing::info!("⏳ Registering with setup token (auto-claim enabled)...");
    } else if device_id.is_some() {
        tracing::info!("⏳ Reconnecting with device ID verification...");
    } else {
        tracing::info!("⏳ Waiting for derived device ID (first registration)...");
    }

    // Track current device ID for deregistration
    let current_device_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let current_device_id_for_loop = current_device_id.clone();

    // Setup shutdown signal handling
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);
    let writer_for_shutdown = writer.clone();
    let device_id_for_shutdown = current_device_id.clone();

    // Spawn signal handler task
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm =
                signal(SignalKind::terminate()).expect("Failed to create SIGTERM handler");
            let mut sigint =
                signal(SignalKind::interrupt()).expect("Failed to create SIGINT handler");

            tokio::select! {
                _ = sigterm.recv() => {
                    tracing::info!("📥 Received SIGTERM, initiating graceful shutdown...");
                }
                _ = sigint.recv() => {
                    tracing::info!("📥 Received SIGINT, initiating graceful shutdown...");
                }
            }
        }

        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("📥 Received Ctrl+C, initiating graceful shutdown...");
        }

        // Send deregister message before shutdown
        if let Some(device_id) = device_id_for_shutdown.lock().await.as_ref() {
            send_deregister(&writer_for_shutdown, device_id, Some("shutdown")).await;
        }

        // Signal shutdown to main loop
        let _ = shutdown_tx.send(());
    });

    // Main message loop
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                tracing::info!("🛑 Shutdown signal received, exiting main loop...");
                break;
            }
            msg_result = read.next() => {
                let msg = match msg_result {
                    Some(Ok(msg)) => msg,
                    Some(Err(e)) => {
                        tracing::error!("❌ WebSocket error: {}", e);
                        break;
                    }
                    None => {
                        tracing::info!("🔌 Connection closed by server");
                        break;
                    }
                };

                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => {
                        tracing::info!("🔌 Connection closed");
                        break;
                    }
                    _ => continue,
                };

                let message: SignalingMessage = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!("⚠️ Invalid message: {}", e);
                        continue;
                    }
                };

                match message {
                    SignalingMessage::Registered {
                        device_id: assigned_id,
                    } => {
                        tracing::info!("✅ Registration confirmed");
                        tracing::info!("🆔 Device ID: {}", assigned_id);
                        tracing::info!("");
                        tracing::info!("📋 To claim ownership:");
                        tracing::info!(
                            "   Anyone with this secret can become an owner (co-ownership supported)"
                        );
                        tracing::info!("");
                        tracing::info!("   WebSocket message:");
                        tracing::info!(
                            r#"   {{ "type": "claim_cocoon", "device_id": "{}", "secret": "{}", "access_token": "YOUR_TOKEN" }}"#,
                            assigned_id,
                            secret_for_claiming
                        );
                        tracing::info!("");
                        tracing::info!("   ⚠️  Share this secret only with trusted co-owners!");
                        tracing::info!("");

                        // Store device ID for deregistration
                        *current_device_id_for_loop.lock().await = Some(assigned_id.clone());

                        // Save device_id for future reconnections (enables verification)
                        save_device_id(&assigned_id).await;
                    }

                    SignalingMessage::RegisteredWithOwner {
                        device_id: assigned_id,
                        owner_id,
                        name,
                    } => {
                        tracing::info!("✅ Registration confirmed with auto-claim");
                        tracing::info!("🆔 Device ID: {}", assigned_id);
                        tracing::info!("👤 Owner: {}", owner_id);
                        if let Some(ref n) = name {
                            tracing::info!("📛 Name: {}", n);
                        }
                        tracing::info!("");
                        tracing::info!("🎉 Cocoon is ready and claimed by your account!");
                        tracing::info!("");

                        // Store device ID for deregistration
                        *current_device_id_for_loop.lock().await = Some(assigned_id.clone());

                        // Save device_id for future reconnections
                        save_device_id(&assigned_id).await;

                        // Clear setup token from env (one-time use)
                        // Note: Can't actually clear env var, but we could delete from config
                    }

                    SignalingMessage::Deregistered { device_id } => {
                        tracing::info!("✅ Deregistration confirmed for device: {}", device_id);
                    }

                    SignalingMessage::SyncData { payload } => {
                        let request: CommandRequest = match serde_json::from_value(payload) {
                            Ok(req) => req,
                            Err(e) => {
                                tracing::warn!("⚠️ Invalid command request: {}", e);
                                continue;
                            }
                        };

                        let writer_clone = writer.clone();
                        let sessions_clone = pty_sessions.clone();
                        let services_clone = services.clone();
                        let silk_sessions_clone = silk_sessions.clone();

                        tokio::spawn(async move {
                            let response: Option<CommandResponse> = match request {
                                CommandRequest::Execute { command, input } => {
                                    tracing::info!("🚀 Executing: {}", command);
                                    Some(execute_command(&command, input.as_deref()).await)
                                }

                                CommandRequest::AttachPty {
                                    command,
                                    cols,
                                    rows,
                                    env,
                                } => {
                                    tracing::info!("🔗 Attaching PTY: {} ({}x{})", command, cols, rows);

                                    match create_pty_session(
                                        &command,
                                        cols,
                                        rows,
                                        &env,
                                        writer_clone.clone(),
                                    )
                                    .await
                                    {
                                        Ok((session_id, session)) => {
                                            sessions_clone.lock().await.insert(session_id, session);
                                            Some(CommandResponse::PtyCreated { session_id })
                                        }
                                        Err(e) => Some(CommandResponse::Error {
                                            code: "pty_create_failed".into(),
                                            message: e,
                                        }),
                                    }
                                }

                                CommandRequest::PtyInput { session_id, data } => {
                                    let mut sessions = sessions_clone.lock().await;
                                    if let Some(session) = sessions.get_mut(&session_id) {
                                        if let Err(e) =
                                            std::io::Write::write_all(&mut session.writer, data.as_bytes())
                                        {
                                            Some(CommandResponse::Error {
                                                code: "pty_write_failed".into(),
                                                message: e.to_string(),
                                            })
                                        } else {
                                            let _ = std::io::Write::flush(&mut session.writer);
                                            None // No response needed for successful input
                                        }
                                    } else {
                                        Some(CommandResponse::Error {
                                            code: "session_not_found".into(),
                                            message: format!("PTY session {} not found", session_id),
                                        })
                                    }
                                }

                                CommandRequest::PtyResize {
                                    session_id,
                                    cols,
                                    rows,
                                } => {
                                    tracing::info!("📐 Resizing PTY {} to {}x{}", session_id, cols, rows);
                                    let sessions = sessions_clone.lock().await;
                                    if let Some(session) = sessions.get(&session_id) {
                                        if let Err(e) = session.pair.master.resize(PtySize {
                                            rows,
                                            cols,
                                            pixel_width: 0,
                                            pixel_height: 0,
                                        }) {
                                            Some(CommandResponse::Error {
                                                code: "resize_failed".into(),
                                                message: e.to_string(),
                                            })
                                        } else {
                                            None // No response needed for successful resize
                                        }
                                    } else {
                                        Some(CommandResponse::Error {
                                            code: "session_not_found".into(),
                                            message: format!("PTY session {} not found", session_id),
                                        })
                                    }
                        }

                        CommandRequest::PtyClose { session_id } => {
                            tracing::info!("🔌 Closing PTY session {}", session_id);
                            let mut sessions = sessions_clone.lock().await;
                            if let Some(mut session) = sessions.remove(&session_id) {
                                let exit_status = session.child.wait().ok();
                                let exit_code =
                                    exit_status.map(|s| s.exit_code() as i32).unwrap_or(-1);

                                Some(CommandResponse::PtyExited {
                                    session_id,
                                    exit_code,
                                })
                            } else {
                                Some(CommandResponse::Error {
                                    code: "session_not_found".into(),
                                    message: format!("PTY session {} not found", session_id),
                                })
                            }
                        }

                        CommandRequest::ProxyHttp {
                            request_id,
                            service_name,
                            method,
                            path,
                            headers,
                            body,
                        } => {
                            tracing::info!(
                                "🔀 Proxying HTTP {} {} to service {}",
                                method,
                                path,
                                service_name
                            );
                            Some(
                                handle_proxy_request(
                                    request_id,
                                    service_name,
                                    method,
                                    path,
                                    headers,
                                    body,
                                    &services_clone,
                                )
                                .await,
                            )
                        }

                        CommandRequest::QueryLocal {
                            query_id,
                            query_type,
                            params,
                        } => {
                            tracing::info!("📊 Processing query: {:?}", query_type);
                            Some(handle_query_local(query_id, query_type, params).await)
                        }

                        // ========== Silk Terminal Commands ==========
                        CommandRequest::SilkCreateSession { cwd, env, shell } => {
                            tracing::info!("🧵 Creating Silk session");
                            match SilkSession::new(cwd, env, shell) {
                                Ok(session) => {
                                    let response = SilkResponse::SessionCreated {
                                        session_id: session.id,
                                        cwd: session.cwd.clone(),
                                        shell: session.shell.clone(),
                                    };
                                    silk_sessions_clone.lock().await.insert(session.id, session);
                                    Some(CommandResponse::SilkResponse(response))
                                }
                                Err(e) => {
                                    Some(CommandResponse::SilkResponse(SilkResponse::Error {
                                        session_id: None,
                                        command_id: None,
                                        code: "session_create_failed".to_string(),
                                        message: e,
                                    }))
                                }
                            }
                        }

                        CommandRequest::SilkExecute {
                            session_id,
                            command,
                            command_id,
                        } => {
                            tracing::info!("🧵 Silk execute: {} (session {})", command, session_id);
                            let mut silk_sessions = silk_sessions_clone.lock().await;

                            if let Some(session) = silk_sessions.get_mut(&session_id) {
                                match session.execute(&command, command_id) {
                                    Ok((interactive, child_opt)) => {
                                        if interactive {
                                            // Need PTY for interactive command
                                            // Create PTY session with the command
                                            drop(silk_sessions); // Release lock before async call

                                            let mut env = HashMap::new();
                                            env.insert(
                                                "TERM".to_string(),
                                                "xterm-256color".to_string(),
                                            );

                                            match create_pty_session(
                                                &command,
                                                80,
                                                24,
                                                &env,
                                                writer_clone.clone(),
                                            )
                                            .await
                                            {
                                                Ok((pty_session_id, pty_session)) => {
                                                    sessions_clone
                                                        .lock()
                                                        .await
                                                        .insert(pty_session_id, pty_session);

                                                    // Update silk session with PTY info
                                                    if let Some(s) = silk_sessions_clone
                                                        .lock()
                                                        .await
                                                        .get_mut(&session_id)
                                                    {
                                                        s.set_pty_session(
                                                            command_id,
                                                            pty_session_id,
                                                        );
                                                    }

                                                    Some(CommandResponse::SilkResponse(
                                                        SilkResponse::InteractiveRequired {
                                                            session_id,
                                                            command_id,
                                                            reason: format!(
                                                                "Command '{}' requires interactive mode",
                                                                command
                                                                    .split_whitespace()
                                                                    .next()
                                                                    .unwrap_or(&command)
                                                            ),
                                                            pty_session_id,
                                                        },
                                                    ))
                                                }
                                                Err(e) => Some(CommandResponse::SilkResponse(
                                                    SilkResponse::Error {
                                                        session_id: Some(session_id),
                                                        command_id: Some(command_id),
                                                        code: "pty_create_failed".to_string(),
                                                        message: e,
                                                    },
                                                )),
                                            }
                                        } else if let Some(mut child) = child_opt {
                                            // Non-interactive command - stream output
                                            let writer_for_output = writer_clone.clone();
                                            let sessions_for_cwd = silk_sessions_clone.clone();
                                            let cmd_for_cwd = command.clone();

                                            // Send started message
                                            let started = SilkResponse::CommandStarted {
                                                session_id,
                                                command_id,
                                                interactive: false,
                                            };
                                            let started_msg = SignalingMessage::SyncData {
                                                payload: serde_json::to_value(
                                                    &CommandResponse::SilkResponse(started),
                                                )
                                                .unwrap(),
                                            };
                                            let mut w = writer_clone.lock().await;
                                            let _ = w
                                                .send(Message::Text(
                                                    serde_json::to_string(&started_msg).unwrap(),
                                                ))
                                                .await;
                                            drop(w);

                                            // Spawn task to read output
                                            tokio::spawn(async move {
                                                let mut stdout_reader = std::io::BufReader::new(
                                                    child.stdout.take().unwrap(),
                                                );
                                                let mut stderr_reader = std::io::BufReader::new(
                                                    child.stderr.take().unwrap(),
                                                );

                                                // Read stdout in chunks
                                                let mut buf = [0u8; 4096];
                                                loop {
                                                    match stdout_reader.get_mut().read(&mut buf) {
                                                        Ok(0) => break,
                                                        Ok(n) => {
                                                            let data =
                                                                String::from_utf8_lossy(&buf[..n])
                                                                    .to_string();
                                                            let html = AnsiToHtml::convert(&data);
                                                            let output = SilkResponse::Output {
                                                                session_id,
                                                                command_id,
                                                                stream: SilkStream::Stdout,
                                                                data: data.clone(),
                                                                html: Some(html),
                                                            };
                                                            let msg = SignalingMessage::SyncData {
                                                                payload: serde_json::to_value(
                                                                    &CommandResponse::SilkResponse(
                                                                        output,
                                                                    ),
                                                                )
                                                                .unwrap(),
                                                            };
                                                            let mut w =
                                                                writer_for_output.lock().await;
                                                            let _ = w
                                                                .send(Message::Text(
                                                                    serde_json::to_string(&msg)
                                                                        .unwrap(),
                                                                ))
                                                                .await;
                                                        }
                                                        Err(_) => break,
                                                    }
                                                }

                                                // Read any remaining stderr
                                                let mut stderr_buf = Vec::new();
                                                let _ = stderr_reader.read_to_end(&mut stderr_buf);
                                                if !stderr_buf.is_empty() {
                                                    let data = String::from_utf8_lossy(&stderr_buf)
                                                        .to_string();
                                                    let html = AnsiToHtml::convert(&data);
                                                    let output = SilkResponse::Output {
                                                        session_id,
                                                        command_id,
                                                        stream: SilkStream::Stderr,
                                                        data: data.clone(),
                                                        html: Some(html),
                                                    };
                                                    let msg = SignalingMessage::SyncData {
                                                        payload: serde_json::to_value(
                                                            &CommandResponse::SilkResponse(output),
                                                        )
                                                        .unwrap(),
                                                    };
                                                    let mut w = writer_for_output.lock().await;
                                                    let _ = w
                                                        .send(Message::Text(
                                                            serde_json::to_string(&msg).unwrap(),
                                                        ))
                                                        .await;
                                                }

                                                // Wait for exit
                                                let exit_code = child
                                                    .wait()
                                                    .map(|s| s.code().unwrap_or(-1))
                                                    .unwrap_or(-1);

                                                // Update cwd if cd command
                                                {
                                                    let mut sessions =
                                                        sessions_for_cwd.lock().await;
                                                    if let Some(s) = sessions.get_mut(&session_id) {
                                                        s.update_cwd_if_cd(&cmd_for_cwd);
                                                        s.complete_command(command_id);

                                                        let completed =
                                                            SilkResponse::CommandCompleted {
                                                                session_id,
                                                                command_id,
                                                                exit_code,
                                                                cwd: s.cwd.clone(),
                                                            };
                                                        let msg = SignalingMessage::SyncData {
                                                            payload: serde_json::to_value(
                                                                &CommandResponse::SilkResponse(
                                                                    completed,
                                                                ),
                                                            )
                                                            .unwrap(),
                                                        };
                                                        let mut w = writer_for_output.lock().await;
                                                        let _ = w
                                                            .send(Message::Text(
                                                                serde_json::to_string(&msg)
                                                                    .unwrap(),
                                                            ))
                                                            .await;
                                                    }
                                                }
                                            });

                                            None // Response sent asynchronously
                                        } else {
                                            Some(CommandResponse::SilkResponse(
                                                SilkResponse::Error {
                                                    session_id: Some(session_id),
                                                    command_id: Some(command_id),
                                                    code: "execute_failed".to_string(),
                                                    message: "No child process created".to_string(),
                                                },
                                            ))
                                        }
                                    }
                                    Err(e) => {
                                        Some(CommandResponse::SilkResponse(SilkResponse::Error {
                                            session_id: Some(session_id),
                                            command_id: Some(command_id),
                                            code: "execute_failed".to_string(),
                                            message: e,
                                        }))
                                    }
                                }
                            } else {
                                Some(CommandResponse::SilkResponse(SilkResponse::Error {
                                    session_id: Some(session_id),
                                    command_id: Some(command_id),
                                    code: "session_not_found".to_string(),
                                    message: format!("Silk session {} not found", session_id),
                                }))
                            }
                        }

                        CommandRequest::SilkInput {
                            session_id,
                            command_id,
                            data,
                        } => {
                            // For interactive commands, forward to PTY
                            let silk_sessions = silk_sessions_clone.lock().await;
                            if let Some(session) = silk_sessions.get(&session_id) {
                                if let Some(cmd) = session.running_commands.get(&command_id) {
                                    if let Some(pty_session_id) = cmd.pty_session_id {
                                        drop(silk_sessions);
                                        let mut pty_sessions = sessions_clone.lock().await;
                                        if let Some(pty) = pty_sessions.get_mut(&pty_session_id) {
                                            if let Err(e) = std::io::Write::write_all(
                                                &mut pty.writer,
                                                data.as_bytes(),
                                            ) {
                                                Some(CommandResponse::SilkResponse(
                                                    SilkResponse::Error {
                                                        session_id: Some(session_id),
                                                        command_id: Some(command_id),
                                                        code: "input_failed".to_string(),
                                                        message: e.to_string(),
                                                    },
                                                ))
                                            } else {
                                                let _ = std::io::Write::flush(&mut pty.writer);
                                                None
                                            }
                                        } else {
                                            Some(CommandResponse::SilkResponse(
                                                SilkResponse::Error {
                                                    session_id: Some(session_id),
                                                    command_id: Some(command_id),
                                                    code: "pty_not_found".to_string(),
                                                    message: "PTY session not found".to_string(),
                                                },
                                            ))
                                        }
                                    } else {
                                        Some(CommandResponse::SilkResponse(SilkResponse::Error {
                                            session_id: Some(session_id),
                                            command_id: Some(command_id),
                                            code: "not_interactive".to_string(),
                                            message: "Command is not in interactive mode"
                                                .to_string(),
                                        }))
                                    }
                                } else {
                                    Some(CommandResponse::SilkResponse(SilkResponse::Error {
                                        session_id: Some(session_id),
                                        command_id: Some(command_id),
                                        code: "command_not_found".to_string(),
                                        message: "Command not found in session".to_string(),
                                    }))
                                }
                            } else {
                                Some(CommandResponse::SilkResponse(SilkResponse::Error {
                                    session_id: Some(session_id),
                                    command_id: Some(command_id),
                                    code: "session_not_found".to_string(),
                                    message: format!("Silk session {} not found", session_id),
                                }))
                            }
                        }

                        CommandRequest::SilkResize {
                            session_id,
                            command_id,
                            cols,
                            rows,
                        } => {
                            let silk_sessions = silk_sessions_clone.lock().await;
                            if let Some(session) = silk_sessions.get(&session_id) {
                                if let Some(cmd) = session.running_commands.get(&command_id) {
                                    if let Some(pty_session_id) = cmd.pty_session_id {
                                        drop(silk_sessions);
                                        let pty_sessions = sessions_clone.lock().await;
                                        if let Some(pty) = pty_sessions.get(&pty_session_id) {
                                            if let Err(e) = pty.pair.master.resize(PtySize {
                                                rows,
                                                cols,
                                                pixel_width: 0,
                                                pixel_height: 0,
                                            }) {
                                                Some(CommandResponse::SilkResponse(
                                                    SilkResponse::Error {
                                                        session_id: Some(session_id),
                                                        command_id: Some(command_id),
                                                        code: "resize_failed".to_string(),
                                                        message: e.to_string(),
                                                    },
                                                ))
                                            } else {
                                                None
                                            }
                                        } else {
                                            None // PTY may have closed already
                                        }
                                    } else {
                                        None // Not interactive, no resize needed
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }

                        CommandRequest::SilkCloseSession { session_id } => {
                            tracing::info!("🧵 Closing Silk session {}", session_id);
                            let mut silk_sessions = silk_sessions_clone.lock().await;
                            if silk_sessions.remove(&session_id).is_some() {
                                Some(CommandResponse::SilkResponse(SilkResponse::SessionClosed {
                                    session_id,
                                }))
                            } else {
                                Some(CommandResponse::SilkResponse(SilkResponse::Error {
                                    session_id: Some(session_id),
                                    command_id: None,
                                    code: "session_not_found".to_string(),
                                    message: format!("Silk session {} not found", session_id),
                                }))
                            }
                        }
                    };

                                // Send response (if any)
                                if let Some(response) = response {
                                    let response_msg = SignalingMessage::SyncData {
                                        payload: serde_json::to_value(&response).unwrap(),
                                    };

                                    let mut w = writer_clone.lock().await;
                                    if let Err(e) = w
                                        .send(Message::Text(serde_json::to_string(&response_msg).unwrap()))
                                        .await
                                    {
                                        tracing::error!("❌ Failed to send response: {}", e);
                                    }
                                }
                            });
                        }

                    SignalingMessage::PeerConnected { peer_id } => {
                        tracing::info!("👋 Peer connected: {}", peer_id);
                    }

                    SignalingMessage::PeerDisconnected { peer_id } => {
                        tracing::info!("👋 Peer disconnected: {}", peer_id);
                    }

                    SignalingMessage::Error { message } => {
                        tracing::error!("❌ Server error: {}", message);
                    }

                    // ========== WebRTC Session Handlers ==========
                    SignalingMessage::WebRtcStartSession {
                        session_id,
                        device_id: client_id,
                        ..
                    } => {
                        tracing::info!("🎥 WebRTC session request from {}: {}", client_id, session_id);
                        
                        // IMPORTANT: Create session synchronously to avoid race condition
                        // The browser sends the offer immediately after this message,
                        // so the session must exist before we process the next message.
                        match webrtc_manager.create_session(session_id.clone()).await {
                            Ok(()) => {
                                tracing::info!("✅ WebRTC session {} created", session_id);
                                // Session started confirmation is sent by signaling server
                            }
                            Err(e) => {
                                tracing::error!("❌ Failed to create WebRTC session: {}", e);
                                let error_msg = SignalingMessage::WebRtcError {
                                    session_id,
                                    code: "session_create_failed".to_string(),
                                    message: e,
                                };
                                let mut w = writer.lock().await;
                                let _ = w.send(Message::Text(serde_json::to_string(&error_msg).unwrap())).await;
                            }
                        }
                    }

                    SignalingMessage::WebRtcOffer { session_id, sdp } => {
                        tracing::info!("📥 WebRTC offer received for session {}", session_id);
                        let webrtc = webrtc_manager.clone();
                        let writer_clone = writer.clone();
                        let session_id_clone = session_id.clone();
                        
                        tokio::spawn(async move {
                            match webrtc.handle_offer(&session_id_clone, &sdp).await {
                                Ok(answer_sdp) => {
                                    tracing::info!("📤 Sending WebRTC answer for session {}", session_id_clone);
                                    let answer_msg = SignalingMessage::WebRtcAnswer {
                                        session_id: session_id_clone,
                                        sdp: answer_sdp,
                                    };
                                    let mut w = writer_clone.lock().await;
                                    let _ = w.send(Message::Text(serde_json::to_string(&answer_msg).unwrap())).await;
                                }
                                Err(e) => {
                                    tracing::error!("❌ Failed to handle WebRTC offer: {}", e);
                                    let error_msg = SignalingMessage::WebRtcError {
                                        session_id: session_id_clone,
                                        code: "offer_failed".to_string(),
                                        message: e,
                                    };
                                    let mut w = writer_clone.lock().await;
                                    let _ = w.send(Message::Text(serde_json::to_string(&error_msg).unwrap())).await;
                                }
                            }
                        });
                    }

                    SignalingMessage::WebRtcIceCandidate {
                        session_id,
                        candidate,
                        sdp_mid,
                        sdp_mline_index,
                    } => {
                        tracing::debug!("🧊 ICE candidate received for session {}", session_id);
                        let webrtc = webrtc_manager.clone();
                        
                        tokio::spawn(async move {
                            if let Err(e) = webrtc
                                .add_ice_candidate(
                                    &session_id,
                                    &candidate,
                                    sdp_mid.as_deref(),
                                    sdp_mline_index,
                                )
                                .await
                            {
                                tracing::warn!("⚠️ Failed to add ICE candidate: {}", e);
                            }
                        });
                    }

                    SignalingMessage::WebRtcSessionEnded { session_id, reason } => {
                        tracing::info!(
                            "🔌 WebRTC session {} ended (reason: {:?})",
                            session_id,
                            reason.as_deref().unwrap_or("not specified")
                        );
                        let webrtc = webrtc_manager.clone();
                        
                        tokio::spawn(async move {
                            let _ = webrtc.close_session(&session_id).await;
                        });
                    }

                    SignalingMessage::WebRtcData {
                        session_id: _,
                        channel,
                        data,
                        binary,
                    } => {
                        // Handle incoming data from WebRTC data channel (via signaling fallback)
                        tracing::debug!("📦 WebRTC data received: {} bytes on channel {}", data.len(), channel);
                        
                        // Process the data based on channel type
                        match channel.as_str() {
                            "terminal" => {
                                // Forward to terminal processing
                                // This could trigger command execution similar to SyncData
                                tracing::debug!("Terminal data: {}", data);
                            }
                            "file-transfer" => {
                                // Handle file transfer
                                tracing::debug!("File transfer data: {} bytes, binary: {}", data.len(), binary);
                            }
                            _ => {
                                tracing::debug!("Unknown channel: {}", channel);
                            }
                        }
                    }

                    SignalingMessage::WebRtcError { session_id, code, message } => {
                        tracing::error!("❌ WebRTC error for session {}: {} - {}", session_id, code, message);
                        let webrtc = webrtc_manager.clone();
                        
                        tokio::spawn(async move {
                            let _ = webrtc.close_session(&session_id).await;
                        });
                    }

                    _ => {
                        tracing::debug!("📨 Other message: {:?}", message);
                    }
                }
            }
        }
    }

    tracing::info!("🐛 Cocoon shutting down");
    Ok(())
}
