use futures::{SinkExt, StreamExt};
use lib_tarminal_sync::SignalingMessage;
use portable_pty::{CommandBuilder, PtySize, PtySystem};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
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

    /// Error response
    Error { code: String, message: String },
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
}

type SharedWriter = Arc<Mutex<futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>>>;

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

    Ok((
        session_id,
        PtySession {
            id: session_id,
            pair,
            child,
        },
    ))
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
        tracing::info!("💾 Saved device ID to {} for reconnection verification", DEVICE_ID_PATH);
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
    tracing::info!("🆕 Generated new cryptographically strong secret ({} chars, {} bits entropy)",
        GENERATED_SECRET_LENGTH, GENERATED_SECRET_LENGTH * 6);

    // Try to save it (may fail in read-only containers, that's ok)
    if let Err(e) = tokio::fs::write(SECRET_PATH, &secret).await {
        tracing::warn!("⚠️ Could not save secret to {} (ephemeral session): {}", SECRET_PATH, e);
        tracing::warn!("💡 Set COCOON_SECRET env var or mount volume at /cocoon for persistent sessions");
    } else {
        tracing::info!("💾 Saved secret to {} for persistent sessions", SECRET_PATH);
    }

    // New secret means no device_id yet (first registration)
    (secret, None)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("cocoon=info".parse().unwrap()),
        )
        .init();

    tracing::info!("🐛 Cocoon starting");

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
    let pty_sessions: Arc<Mutex<HashMap<Uuid, PtySession>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Register with signaling server using secret
    // Server derives deterministic device_id from secret (persistent sessions)
    // Send device_id on reconnect for verification (prevents secret theft attacks)
    let register_msg = SignalingMessage::Register {
        secret,
        device_id: device_id.clone(),
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

    if device_id.is_some() {
        tracing::info!("⏳ Reconnecting with device ID verification...");
    } else {
        tracing::info!("⏳ Waiting for derived device ID (first registration)...");
    }

    // Main message loop
    while let Some(msg_result) = read.next().await {
        let msg = match msg_result {
            Ok(msg) => msg,
            Err(e) => {
                tracing::error!("❌ WebSocket error: {}", e);
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
            SignalingMessage::Registered { device_id: assigned_id } => {
                tracing::info!("✅ Registration confirmed");
                tracing::info!("🆔 Device ID: {}", assigned_id);

                // Save device_id for future reconnections (enables verification)
                save_device_id(&assigned_id).await;
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

                tokio::spawn(async move {
                    let response = match request {
                        CommandRequest::Execute { command, input } => {
                            tracing::info!("🚀 Executing: {}", command);
                            execute_command(&command, input.as_deref()).await
                        }

                        CommandRequest::AttachPty { command, cols, rows, env } => {
                            tracing::info!("🔗 Attaching PTY: {} ({}x{})", command, cols, rows);

                            match create_pty_session(&command, cols, rows, &env, writer_clone.clone()).await {
                                Ok((session_id, session)) => {
                                    sessions_clone.lock().await.insert(session_id, session);
                                    CommandResponse::PtyCreated { session_id }
                                }
                                Err(e) => CommandResponse::Error {
                                    code: "pty_create_failed".into(),
                                    message: e,
                                },
                            }
                        }

                        CommandRequest::PtyInput { session_id, data } => {
                            let sessions = sessions_clone.lock().await;
                            if let Some(session) = sessions.get(&session_id) {
                                let mut writer = session.pair.master.take_writer().unwrap();
                                if let Err(e) = std::io::Write::write_all(&mut writer, data.as_bytes()) {
                                    CommandResponse::Error {
                                        code: "pty_write_failed".into(),
                                        message: e.to_string(),
                                    }
                                } else {
                                    continue; // No response needed for input
                                }
                            } else {
                                CommandResponse::Error {
                                    code: "session_not_found".into(),
                                    message: format!("PTY session {} not found", session_id),
                                }
                            }
                        }

                        CommandRequest::PtyResize { session_id, cols, rows } => {
                            tracing::info!("📐 Resizing PTY {} to {}x{}", session_id, cols, rows);
                            let sessions = sessions_clone.lock().await;
                            if let Some(session) = sessions.get(&session_id) {
                                if let Err(e) = session.pair.master.resize(PtySize {
                                    rows,
                                    cols,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                }) {
                                    CommandResponse::Error {
                                        code: "resize_failed".into(),
                                        message: e.to_string(),
                                    }
                                } else {
                                    continue; // No response needed for resize
                                }
                            } else {
                                CommandResponse::Error {
                                    code: "session_not_found".into(),
                                    message: format!("PTY session {} not found", session_id),
                                }
                            }
                        }

                        CommandRequest::PtyClose { session_id } => {
                            tracing::info!("🔌 Closing PTY session {}", session_id);
                            let mut sessions = sessions_clone.lock().await;
                            if let Some(mut session) = sessions.remove(&session_id) {
                                let exit_status = session.child.wait().ok();
                                let exit_code = exit_status
                                    .and_then(|s| s.exit_code())
                                    .unwrap_or(-1) as i32;

                                CommandResponse::PtyExited {
                                    session_id,
                                    exit_code,
                                }
                            } else {
                                CommandResponse::Error {
                                    code: "session_not_found".into(),
                                    message: format!("PTY session {} not found", session_id),
                                }
                            }
                        }
                    };

                    // Send response
                    let response_msg = SignalingMessage::SyncData {
                        payload: serde_json::to_value(&response).unwrap(),
                    };

                    let mut w = writer_clone.lock().await;
                    if let Err(e) = w
                        .send(Message::Text(
                            serde_json::to_string(&response_msg).unwrap(),
                        ))
                        .await
                    {
                        tracing::error!("❌ Failed to send response: {}", e);
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

            _ => {
                tracing::debug!("📨 Other message: {:?}", message);
            }
        }
    }

    tracing::info!("🐛 Cocoon shutting down");
}
