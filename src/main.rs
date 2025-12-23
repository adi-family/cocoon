use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

const INPUT_PATH: &str = "/adi/input/request.json";
const OUTPUT_DIR: &str = "/adi/output";
const DEFAULT_PORT: u16 = 8080;

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
    code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<String>,
}

#[derive(Debug, Serialize)]
struct OutputFile {
    path: String,
    content: String,
    binary: bool,
}

fn json_response(status: StatusCode, body: impl Serialize) -> Response<Full<Bytes>> {
    let body = serde_json::to_vec(&body).unwrap_or_default();
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Full::new(Bytes::from(body)))
        .unwrap()
}

async fn handle_health() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from_static(b"OK")))
        .unwrap()
}

async fn handle_execute() -> Response<Full<Bytes>> {
    // Read request from input file
    let request: WorkerRequest = match tokio::fs::read_to_string(INPUT_PATH).await {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(req) => req,
            Err(e) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    WorkerResponse {
                        success: false,
                        data: None,
                        error: Some(ErrorInfo {
                            code: "invalid_request",
                            details: Some(e.to_string()),
                        }),
                        files: vec![],
                    },
                );
            }
        },
        Err(e) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                WorkerResponse {
                    success: false,
                    data: None,
                    error: Some(ErrorInfo {
                        code: "input_read_failed",
                        details: Some(e.to_string()),
                    }),
                    files: vec![],
                },
            );
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
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                WorkerResponse {
                    success: false,
                    data: None,
                    error: Some(ErrorInfo {
                        code: "spawn_failed",
                        details: Some(e.to_string()),
                    }),
                    files: vec![],
                },
            );
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
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                WorkerResponse {
                    success: false,
                    data: None,
                    error: Some(ErrorInfo {
                        code: "execution_failed",
                        details: Some(e.to_string()),
                    }),
                    files: vec![],
                },
            );
        }
    };

    // Collect output files
    let files = collect_output_files(OUTPUT_DIR).await;

    // Build response
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        json_response(
            StatusCode::OK,
            WorkerResponse {
                success: true,
                data: Some(serde_json::json!({
                    "stdout": stdout,
                    "stderr": stderr,
                    "exit_code": 0
                })),
                error: None,
                files,
            },
        )
    } else {
        let exit_code = output.status.code().unwrap_or(-1);
        json_response(
            StatusCode::OK,
            WorkerResponse {
                success: false,
                data: Some(serde_json::json!({
                    "stdout": stdout,
                    "stderr": stderr,
                    "exit_code": exit_code
                })),
                error: Some(ErrorInfo {
                    code: "command_failed",
                    details: Some(format!("exit code: {}", exit_code)),
                }),
                files,
            },
        )
    }
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
    {
        let path = entry.path();
        let rel_path = path
            .strip_prefix(dir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());

        match tokio::fs::read(path).await {
            Ok(content) => {
                let is_binary = content.iter().any(|&b| b == 0);
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

async fn handle_request(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::GET, "/health") => handle_health().await,
        (&Method::POST, "/execute") => handle_execute().await,
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from_static(b"Not Found")))
            .unwrap(),
    };
    Ok(response)
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("WORKER_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let addr = format!("0.0.0.0:{}", port);

    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind {}: {}", addr, e);
            std::process::exit(1);
        }
    };

    eprintln!("Worker listening on {}", addr);

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("Accept error: {}", e);
                continue;
            }
        };

        let io = TokioIo::new(stream);

        tokio::spawn(async move {
            if let Err(e) = http1::Builder::new()
                .serve_connection(io, service_fn(handle_request))
                .await
            {
                eprintln!("Connection error: {}", e);
            }
        });
    }
}
