use futures::{SinkExt, StreamExt};
use lib_tarminal_sync::SignalingMessage;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

const OUTPUT_DIR: &str = "/cocoon/output";
const RESPONSE_PATH: &str = "/cocoon/output/response.json";

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CommandRequest {
    Execute { command: String, input: Option<String> },
}

#[derive(Debug, Serialize)]
struct CommandResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorInfo>,
    #[serde(default)]
    files: Vec<OutputFile>,
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
    // Create output directory
    let _ = tokio::fs::create_dir_all(OUTPUT_DIR).await;

    // Execute command
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
            return CommandResponse {
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

    // Write input to stdin if provided
    if let Some(input_str) = input {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input_str.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
    }

    // Wait for process
    let output = match child.wait_with_output().await {
        Ok(output) => output,
        Err(e) => {
            return CommandResponse {
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

    // Collect output files
    let files = collect_output_files(OUTPUT_DIR).await;

    // Build response
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        CommandResponse {
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
        CommandResponse {
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

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("cocoon=info".parse().unwrap()),
        )
        .init();

    tracing::info!("🐛 Cocoon starting");

    // Get signaling server URL from environment
    let signaling_url = std::env::var("SIGNALING_SERVER_URL")
        .unwrap_or_else(|_| "ws://localhost:8080/ws".to_string());

    // Generate or get device ID
    let device_id = std::env::var("COCOON_ID").unwrap_or_else(|_| Uuid::new_v4().to_string());

    tracing::info!("🔗 Connecting to signaling server: {}", signaling_url);
    tracing::info!("🆔 Device ID: {}", device_id);

    // Connect to signaling server
    let (ws_stream, _) = match connect_async(&signaling_url).await {
        Ok(conn) => conn,
        Err(e) => {
            tracing::error!("❌ Failed to connect to signaling server: {}", e);
            std::process::exit(1);
        }
    };

    let (mut write, mut read) = ws_stream.split();

    // Register with signaling server
    let register_msg = SignalingMessage::Register {
        device_id: device_id.clone(),
    };

    if let Err(e) = write
        .send(Message::Text(serde_json::to_string(&register_msg).unwrap()))
        .await
    {
        tracing::error!("❌ Failed to register: {}", e);
        std::process::exit(1);
    }

    tracing::info!("✅ Connected and waiting for commands...");

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
            SignalingMessage::Registered { .. } => {
                tracing::info!("✅ Registration confirmed");
            }

            SignalingMessage::SyncData { payload } => {
                tracing::info!("📥 Received command request");

                // Parse command request from payload
                let request: CommandRequest = match serde_json::from_value(payload) {
                    Ok(req) => req,
                    Err(e) => {
                        tracing::warn!("⚠️ Invalid command request: {}", e);
                        continue;
                    }
                };

                match request {
                    CommandRequest::Execute { command, input } => {
                        tracing::info!("🚀 Executing: {}", command);

                        let response = execute_command(&command, input.as_deref()).await;

                        // Send response back via signaling server
                        let response_msg = SignalingMessage::SyncData {
                            payload: serde_json::to_value(&response).unwrap(),
                        };

                        if let Err(e) = write
                            .send(Message::Text(
                                serde_json::to_string(&response_msg).unwrap(),
                            ))
                            .await
                        {
                            tracing::error!("❌ Failed to send response: {}", e);
                        } else {
                            tracing::info!("📤 Response sent");
                        }
                    }
                }
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
