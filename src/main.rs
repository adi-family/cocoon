use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

const INPUT_PATH: &str = "/adi/input/request.json";
const OUTPUT_DIR: &str = "/adi/output";
const RESPONSE_PATH: &str = "/adi/output/response.json";

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkerRequest {
    Message { message: String },
}

#[derive(Debug, Serialize)]
struct WorkerResponse {
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

fn write_response_sync(response: &WorkerResponse) {
    let json = serde_json::to_string_pretty(response).unwrap_or_else(|_| "{}".into());
    let _ = std::fs::write(RESPONSE_PATH, json);
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

async fn run() -> WorkerResponse {
    // Read request from input file
    let request: WorkerRequest = match tokio::fs::read_to_string(INPUT_PATH).await {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(req) => req,
            Err(e) => {
                return WorkerResponse {
                    success: false,
                    data: None,
                    error: Some(ErrorInfo {
                        code: "invalid_request".into(),
                        details: Some(e.to_string()),
                    }),
                    files: vec![],
                };
            }
        },
        Err(e) => {
            return WorkerResponse {
                success: false,
                data: None,
                error: Some(ErrorInfo {
                    code: "input_read_failed".into(),
                    details: Some(e.to_string()),
                }),
                files: vec![],
            };
        }
    };

    let WorkerRequest::Message { message } = request;

    // Get command from env
    let cmd = std::env::var("ORIGINAL_CMD").unwrap_or_else(|_| "/bin/sh".into());

    // Create output directory
    let _ = tokio::fs::create_dir_all(OUTPUT_DIR).await;

    // Execute command
    let mut child = match tokio::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            return WorkerResponse {
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

    // Write message to stdin
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(message.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }

    // Wait for process
    let output = match child.wait_with_output().await {
        Ok(output) => output,
        Err(e) => {
            return WorkerResponse {
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
        WorkerResponse {
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
        WorkerResponse {
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
    eprintln!("adi-worker starting");

    let response = run().await;
    let success = response.success;

    // Write response to file
    let json = serde_json::to_string_pretty(&response).unwrap_or_else(|_| "{}".into());
    if let Err(e) = tokio::fs::write(RESPONSE_PATH, &json).await {
        eprintln!("Failed to write response: {}", e);
        // Try sync write as fallback
        write_response_sync(&response);
    }

    eprintln!("adi-worker completed, success={}", success);

    // Exit with appropriate code
    std::process::exit(if success { 0 } else { 1 });
}
