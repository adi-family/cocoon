//! Tools Service - ADI service for tool execution
//!
//! Provides a unified interface for executing tools from various sources:
//! - Built-in tools (shell, file operations, etc.)
//! - MCP servers (via stdio/SSE transport)
//! - Custom tool providers
//!
//! Tools follow the MCP tool schema format with JSON Schema for parameters.

use crate::adi_router::{AdiHandleResult, AdiService, AdiServiceError};
use async_trait::async_trait;
use lib_signaling_protocol::AdiMethodInfo;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::RwLock;

// ============================================================================
// Tool Types
// ============================================================================

/// Tool definition following MCP schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    /// Unique tool name (e.g., "shell_execute", "file_read")
    pub name: String,

    /// Human-readable description
    pub description: String,

    /// JSON Schema for tool parameters
    pub input_schema: JsonValue,

    /// Tool category for organization
    #[serde(default)]
    pub category: ToolCategory,

    /// Source of the tool (for display purposes)
    #[serde(default)]
    pub source: String,
}

/// Tool execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Result content (text or JSON string)
    pub content: String,

    /// Content type
    pub content_type: ToolContentType,

    /// Whether this is an error result
    pub is_error: bool,

    /// Execution duration in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl ToolResult {
    /// Create a successful text result
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            content_type: ToolContentType::Text,
            is_error: false,
            duration_ms: None,
        }
    }

    /// Create a successful JSON result
    pub fn json<T: Serialize>(data: &T) -> Result<Self, serde_json::Error> {
        Ok(Self {
            content: serde_json::to_string(data)?,
            content_type: ToolContentType::Json,
            is_error: false,
            duration_ms: None,
        })
    }

    /// Create an error result
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: message.into(),
            content_type: ToolContentType::Text,
            is_error: true,
            duration_ms: None,
        }
    }

    /// Set execution duration
    pub fn with_duration(mut self, ms: u64) -> Self {
        self.duration_ms = Some(ms);
        self
    }
}

/// Tool content type
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolContentType {
    #[default]
    Text,
    Json,
}

/// Tool category for organization
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    #[default]
    General,
    FileSystem,
    Shell,
    Network,
    Database,
    Search,
    Transform,
    External,
}

// ============================================================================
// Tool Provider Trait
// ============================================================================

/// Trait for tool providers
///
/// Implement this trait to add custom tools to the ToolsService.
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Provider identifier
    fn provider_id(&self) -> &str;

    /// List all tools provided
    fn list_tools(&self) -> Vec<ToolDef>;

    /// Execute a tool by name
    async fn call_tool(&self, name: &str, arguments: JsonValue) -> Result<ToolResult, String>;

    /// Check if this provider handles a specific tool
    fn handles_tool(&self, name: &str) -> bool {
        self.list_tools().iter().any(|t| t.name == name)
    }
}

// ============================================================================
// Built-in Tool Providers
// ============================================================================

/// Built-in shell tools
pub struct ShellToolProvider {
    /// Working directory for commands
    pub working_dir: Option<String>,
}

impl Default for ShellToolProvider {
    fn default() -> Self {
        Self { working_dir: None }
    }
}

#[async_trait]
impl ToolProvider for ShellToolProvider {
    fn provider_id(&self) -> &str {
        "builtin.shell"
    }

    fn list_tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "shell_execute".to_string(),
                description: "Execute a shell command and return its output".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["command"],
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The shell command to execute"
                        },
                        "working_dir": {
                            "type": "string",
                            "description": "Working directory for the command"
                        },
                        "timeout_ms": {
                            "type": "integer",
                            "description": "Timeout in milliseconds (default: 30000)"
                        }
                    }
                }),
                category: ToolCategory::Shell,
                source: "builtin".to_string(),
            },
            ToolDef {
                name: "shell_which".to_string(),
                description: "Find the path of an executable".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["program"],
                    "properties": {
                        "program": {
                            "type": "string",
                            "description": "Name of the program to find"
                        }
                    }
                }),
                category: ToolCategory::Shell,
                source: "builtin".to_string(),
            },
        ]
    }

    async fn call_tool(&self, name: &str, arguments: JsonValue) -> Result<ToolResult, String> {
        match name {
            "shell_execute" => {
                let command = arguments
                    .get("command")
                    .and_then(|v| v.as_str())
                    .ok_or("command is required")?;

                let working_dir = arguments
                    .get("working_dir")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or_else(|| self.working_dir.clone());

                let timeout_ms = arguments
                    .get("timeout_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(30000);

                let start = std::time::Instant::now();

                let mut cmd = Command::new("sh");
                cmd.arg("-c").arg(command);

                if let Some(dir) = working_dir {
                    cmd.current_dir(dir);
                }

                let output = tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    cmd.output(),
                )
                .await
                .map_err(|_| "Command timed out")?
                .map_err(|e| format!("Failed to execute command: {}", e))?;

                let duration_ms = start.elapsed().as_millis() as u64;

                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                let result = json!({
                    "exit_code": output.status.code(),
                    "stdout": stdout,
                    "stderr": stderr,
                    "success": output.status.success()
                });

                Ok(ToolResult::json(&result)
                    .map_err(|e| e.to_string())?
                    .with_duration(duration_ms))
            }

            "shell_which" => {
                let program = arguments
                    .get("program")
                    .and_then(|v| v.as_str())
                    .ok_or("program is required")?;

                let output = Command::new("which")
                    .arg(program)
                    .output()
                    .await
                    .map_err(|e| format!("Failed to run which: {}", e))?;

                if output.status.success() {
                    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    Ok(ToolResult::json(&json!({ "path": path, "found": true }))
                        .map_err(|e| e.to_string())?)
                } else {
                    Ok(ToolResult::json(&json!({ "path": null, "found": false }))
                        .map_err(|e| e.to_string())?)
                }
            }

            _ => Err(format!("Unknown tool: {}", name)),
        }
    }
}

/// Built-in file system tools
pub struct FileSystemToolProvider {
    /// Base directory for file operations (sandbox)
    pub base_dir: Option<String>,
}

impl Default for FileSystemToolProvider {
    fn default() -> Self {
        Self { base_dir: None }
    }
}

#[async_trait]
impl ToolProvider for FileSystemToolProvider {
    fn provider_id(&self) -> &str {
        "builtin.filesystem"
    }

    fn list_tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "file_read".to_string(),
                description: "Read the contents of a file".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["path"],
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file to read"
                        },
                        "encoding": {
                            "type": "string",
                            "enum": ["utf8", "base64"],
                            "default": "utf8",
                            "description": "Encoding for the file content"
                        }
                    }
                }),
                category: ToolCategory::FileSystem,
                source: "builtin".to_string(),
            },
            ToolDef {
                name: "file_write".to_string(),
                description: "Write content to a file".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["path", "content"],
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file to write"
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write"
                        },
                        "encoding": {
                            "type": "string",
                            "enum": ["utf8", "base64"],
                            "default": "utf8",
                            "description": "Encoding of the content"
                        }
                    }
                }),
                category: ToolCategory::FileSystem,
                source: "builtin".to_string(),
            },
            ToolDef {
                name: "file_list".to_string(),
                description: "List files in a directory".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["path"],
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory path to list"
                        },
                        "recursive": {
                            "type": "boolean",
                            "default": false,
                            "description": "List files recursively"
                        },
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern to filter files"
                        }
                    }
                }),
                category: ToolCategory::FileSystem,
                source: "builtin".to_string(),
            },
            ToolDef {
                name: "file_stat".to_string(),
                description: "Get file or directory metadata".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["path"],
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file or directory"
                        }
                    }
                }),
                category: ToolCategory::FileSystem,
                source: "builtin".to_string(),
            },
        ]
    }

    async fn call_tool(&self, name: &str, arguments: JsonValue) -> Result<ToolResult, String> {
        match name {
            "file_read" => {
                let path = arguments
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or("path is required")?;

                let encoding = arguments
                    .get("encoding")
                    .and_then(|v| v.as_str())
                    .unwrap_or("utf8");

                let content = tokio::fs::read(path)
                    .await
                    .map_err(|e| format!("Failed to read file: {}", e))?;

                match encoding {
                    "base64" => {
                        use base64::Engine;
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&content);
                        Ok(ToolResult::json(&json!({
                            "content": encoded,
                            "encoding": "base64",
                            "size": content.len()
                        }))
                        .map_err(|e| e.to_string())?)
                    }
                    _ => {
                        let text = String::from_utf8(content)
                            .map_err(|_| "File is not valid UTF-8, use base64 encoding")?;
                        Ok(ToolResult::json(&json!({
                            "content": text,
                            "encoding": "utf8",
                            "size": text.len()
                        }))
                        .map_err(|e| e.to_string())?)
                    }
                }
            }

            "file_write" => {
                let path = arguments
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or("path is required")?;

                let content = arguments
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or("content is required")?;

                let encoding = arguments
                    .get("encoding")
                    .and_then(|v| v.as_str())
                    .unwrap_or("utf8");

                let bytes = match encoding {
                    "base64" => {
                        use base64::Engine;
                        base64::engine::general_purpose::STANDARD
                            .decode(content)
                            .map_err(|e| format!("Invalid base64: {}", e))?
                    }
                    _ => content.as_bytes().to_vec(),
                };

                tokio::fs::write(path, &bytes)
                    .await
                    .map_err(|e| format!("Failed to write file: {}", e))?;

                Ok(ToolResult::json(&json!({
                    "success": true,
                    "path": path,
                    "bytes_written": bytes.len()
                }))
                .map_err(|e| e.to_string())?)
            }

            "file_list" => {
                let path = arguments
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or("path is required")?;

                let recursive = arguments
                    .get("recursive")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let pattern = arguments.get("pattern").and_then(|v| v.as_str());

                let mut entries = Vec::new();

                if recursive {
                    for entry in walkdir::WalkDir::new(path)
                        .max_depth(10)
                        .into_iter()
                        .filter_map(|e| e.ok())
                    {
                        let entry_path = entry.path().display().to_string();

                        // Apply glob pattern if specified
                        if let Some(pat) = pattern {
                            let glob_pattern =
                                glob::Pattern::new(pat).map_err(|e| e.to_string())?;
                            if !glob_pattern.matches(&entry_path) {
                                continue;
                            }
                        }

                        entries.push(json!({
                            "path": entry_path,
                            "is_dir": entry.file_type().is_dir(),
                            "is_file": entry.file_type().is_file(),
                        }));
                    }
                } else {
                    let mut read_dir = tokio::fs::read_dir(path)
                        .await
                        .map_err(|e| format!("Failed to read directory: {}", e))?;

                    while let Some(entry) = read_dir.next_entry().await.map_err(|e| e.to_string())?
                    {
                        let entry_path = entry.path().display().to_string();
                        let metadata = entry.metadata().await.ok();

                        // Apply glob pattern if specified
                        if let Some(pat) = pattern {
                            let glob_pattern =
                                glob::Pattern::new(pat).map_err(|e| e.to_string())?;
                            if !glob_pattern.matches(&entry_path) {
                                continue;
                            }
                        }

                        entries.push(json!({
                            "path": entry_path,
                            "is_dir": metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                            "is_file": metadata.as_ref().map(|m| m.is_file()).unwrap_or(false),
                            "size": metadata.as_ref().map(|m| m.len()),
                        }));
                    }
                }

                Ok(ToolResult::json(&json!({
                    "path": path,
                    "entries": entries,
                    "count": entries.len()
                }))
                .map_err(|e| e.to_string())?)
            }

            "file_stat" => {
                let path = arguments
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or("path is required")?;

                let metadata = tokio::fs::metadata(path)
                    .await
                    .map_err(|e| format!("Failed to stat: {}", e))?;

                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs());

                let created = metadata
                    .created()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs());

                Ok(ToolResult::json(&json!({
                    "path": path,
                    "is_file": metadata.is_file(),
                    "is_dir": metadata.is_dir(),
                    "is_symlink": metadata.is_symlink(),
                    "size": metadata.len(),
                    "modified": modified,
                    "created": created,
                }))
                .map_err(|e| e.to_string())?)
            }

            _ => Err(format!("Unknown tool: {}", name)),
        }
    }
}

// ============================================================================
// MCP Server Tool Provider
// ============================================================================

/// Tool provider that connects to an MCP server via stdio
pub struct McpServerProvider {
    /// Provider identifier
    id: String,
    /// Command to start the MCP server
    command: String,
    /// Arguments for the command
    args: Vec<String>,
    /// Cached tool list
    tools: RwLock<Vec<ToolDef>>,
}

impl McpServerProvider {
    /// Create a new MCP server provider
    pub fn new(id: impl Into<String>, command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            id: id.into(),
            command: command.into(),
            args,
            tools: RwLock::new(Vec::new()),
        }
    }

    /// Initialize the provider by querying the MCP server for tools
    pub async fn initialize(&self) -> Result<(), String> {
        // Start the MCP server process
        let mut child = Command::new(&self.command)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to start MCP server: {}", e))?;

        let mut stdin = child.stdin.take().ok_or("Failed to get stdin")?;
        let stdout = child.stdout.take().ok_or("Failed to get stdout")?;
        let mut reader = BufReader::new(stdout);

        // Send initialize request
        let init_request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "cocoon-tools",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        });

        stdin
            .write_all(format!("{}\n", init_request).as_bytes())
            .await
            .map_err(|e| e.to_string())?;

        // Read initialize response
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .map_err(|e| e.to_string())?;

        // Send tools/list request
        let list_request = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        });

        stdin
            .write_all(format!("{}\n", list_request).as_bytes())
            .await
            .map_err(|e| e.to_string())?;

        // Read tools/list response
        line.clear();
        reader
            .read_line(&mut line)
            .await
            .map_err(|e| e.to_string())?;

        let response: JsonValue = serde_json::from_str(&line).map_err(|e| e.to_string())?;

        if let Some(tools_array) = response
            .get("result")
            .and_then(|r| r.get("tools"))
            .and_then(|t| t.as_array())
        {
            let mut tools = self.tools.write().await;
            tools.clear();

            for tool in tools_array {
                if let (Some(name), Some(description)) = (
                    tool.get("name").and_then(|n| n.as_str()),
                    tool.get("description").and_then(|d| d.as_str()),
                ) {
                    tools.push(ToolDef {
                        name: name.to_string(),
                        description: description.to_string(),
                        input_schema: tool
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or(json!({"type": "object"})),
                        category: ToolCategory::External,
                        source: self.id.clone(),
                    });
                }
            }
        }

        // Kill the process (we'll spawn new ones for each call)
        let _ = child.kill().await;

        Ok(())
    }
}

#[async_trait]
impl ToolProvider for McpServerProvider {
    fn provider_id(&self) -> &str {
        &self.id
    }

    fn list_tools(&self) -> Vec<ToolDef> {
        // Return cached tools (must call initialize first)
        // Note: This is safe because tools are only modified during initialize()
        // which must be called before the provider is used.
        // We use try_read to avoid blocking - if locked, return empty (unlikely in practice)
        self.tools
            .try_read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    async fn call_tool(&self, name: &str, arguments: JsonValue) -> Result<ToolResult, String> {
        // Start the MCP server process
        let mut child = Command::new(&self.command)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to start MCP server: {}", e))?;

        let mut stdin = child.stdin.take().ok_or("Failed to get stdin")?;
        let stdout = child.stdout.take().ok_or("Failed to get stdout")?;
        let mut reader = BufReader::new(stdout);

        // Send initialize request
        let init_request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "cocoon-tools",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        });

        stdin
            .write_all(format!("{}\n", init_request).as_bytes())
            .await
            .map_err(|e| e.to_string())?;

        // Read initialize response
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .map_err(|e| e.to_string())?;

        // Send tools/call request
        let call_request = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments
            }
        });

        let start = std::time::Instant::now();

        stdin
            .write_all(format!("{}\n", call_request).as_bytes())
            .await
            .map_err(|e| e.to_string())?;

        // Read tools/call response
        line.clear();
        reader
            .read_line(&mut line)
            .await
            .map_err(|e| e.to_string())?;

        let duration_ms = start.elapsed().as_millis() as u64;

        let response: JsonValue = serde_json::from_str(&line).map_err(|e| e.to_string())?;

        // Kill the process
        let _ = child.kill().await;

        // Parse response
        if let Some(error) = response.get("error") {
            return Ok(ToolResult::error(
                error
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown error"),
            ));
        }

        if let Some(result) = response.get("result") {
            let is_error = result.get("isError").and_then(|e| e.as_bool()).unwrap_or(false);
            let content = result
                .get("content")
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.first())
                .and_then(|item| item.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");

            if is_error {
                Ok(ToolResult::error(content).with_duration(duration_ms))
            } else {
                Ok(ToolResult::text(content).with_duration(duration_ms))
            }
        } else {
            Ok(ToolResult::error("Invalid response from MCP server"))
        }
    }
}

// ============================================================================
// Tools Service
// ============================================================================

/// Tools service for ADI router
///
/// Aggregates tools from multiple providers and exposes them via the ADI protocol.
pub struct ToolsService {
    providers: Vec<Arc<dyn ToolProvider>>,
    // Using std::sync::RwLock for synchronous access during construction
    tool_to_provider: std::sync::RwLock<HashMap<String, usize>>,
}

impl Default for ToolsService {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolsService {
    /// Create a new tools service with default built-in providers
    pub fn new() -> Self {
        let mut service = Self {
            providers: Vec::new(),
            tool_to_provider: std::sync::RwLock::new(HashMap::new()),
        };

        // Register built-in providers
        service.add_provider(Arc::new(ShellToolProvider::default()));
        service.add_provider(Arc::new(FileSystemToolProvider::default()));

        service
    }

    /// Create a minimal tools service without any providers
    pub fn minimal() -> Self {
        Self {
            providers: Vec::new(),
            tool_to_provider: std::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Add a tool provider
    pub fn add_provider(&mut self, provider: Arc<dyn ToolProvider>) {
        let provider_idx = self.providers.len();
        self.providers.push(provider.clone());

        // Build tool -> provider index
        let tool_mapping: HashMap<String, usize> = provider
            .list_tools()
            .into_iter()
            .map(|t| (t.name, provider_idx))
            .collect();

        // Update the tool mapping (synchronous)
        if let Ok(mut map) = self.tool_to_provider.write() {
            map.extend(tool_mapping);
        }

        tracing::info!(
            "Registered tool provider: {} with {} tools",
            provider.provider_id(),
            provider.list_tools().len()
        );
    }

    /// Get all available tools
    pub fn list_all_tools(&self) -> Vec<ToolDef> {
        self.providers
            .iter()
            .flat_map(|p| p.list_tools())
            .collect()
    }

    /// Handle list method
    async fn handle_list(&self, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let category_filter = params.get("category").and_then(|v| v.as_str());

        let tools: Vec<ToolDef> = self
            .list_all_tools()
            .into_iter()
            .filter(|t| {
                if let Some(cat) = category_filter {
                    let tool_cat = serde_json::to_string(&t.category).unwrap_or_default();
                    tool_cat.trim_matches('"') == cat
                } else {
                    true
                }
            })
            .collect();

        Ok(AdiHandleResult::Success(json!(tools)))
    }

    /// Handle call method
    async fn handle_call(&self, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdiServiceError::invalid_params("name is required"))?;

        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or(json!({}));

        // Find the provider for this tool (synchronous read)
        let provider_idx = self
            .tool_to_provider
            .read()
            .ok()
            .and_then(|map| map.get(name).copied());

        let provider_idx =
            provider_idx.ok_or_else(|| AdiServiceError::not_found(format!("Tool not found: {}", name)))?;

        let provider = &self.providers[provider_idx];

        // Call the tool
        match provider.call_tool(name, arguments).await {
            Ok(result) => Ok(AdiHandleResult::Success(json!(result))),
            Err(e) => Ok(AdiHandleResult::Success(json!(ToolResult::error(e)))),
        }
    }

    /// Handle get_schema method
    async fn handle_get_schema(
        &self,
        params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdiServiceError::invalid_params("name is required"))?;

        let tool = self
            .list_all_tools()
            .into_iter()
            .find(|t| t.name == name)
            .ok_or_else(|| AdiServiceError::not_found(format!("Tool not found: {}", name)))?;

        Ok(AdiHandleResult::Success(json!({
            "name": tool.name,
            "description": tool.description,
            "input_schema": tool.input_schema,
            "category": tool.category,
            "source": tool.source
        })))
    }

    /// Handle providers method
    async fn handle_providers(&self, _params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let providers: Vec<JsonValue> = self
            .providers
            .iter()
            .map(|p| {
                json!({
                    "id": p.provider_id(),
                    "tool_count": p.list_tools().len()
                })
            })
            .collect();

        Ok(AdiHandleResult::Success(json!(providers)))
    }
}

#[async_trait]
impl AdiService for ToolsService {
    fn service_id(&self) -> &str {
        "tools"
    }

    fn name(&self) -> &str {
        "Tool Execution Service"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn methods(&self) -> Vec<AdiMethodInfo> {
        vec![
            AdiMethodInfo {
                name: "list".to_string(),
                description: "List all available tools".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "category": {
                            "type": "string",
                            "enum": ["general", "file_system", "shell", "network", "database", "search", "transform", "external"],
                            "description": "Filter by category"
                        }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "call".to_string(),
                description: "Execute a tool by name".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["name"],
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Name of the tool to execute"
                        },
                        "arguments": {
                            "type": "object",
                            "description": "Arguments for the tool (see tool's input_schema)"
                        }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "get_schema".to_string(),
                description: "Get the input schema for a specific tool".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["name"],
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Name of the tool"
                        }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "providers".to_string(),
                description: "List all registered tool providers".to_string(),
                streaming: false,
                params_schema: None,
                ..Default::default()
            },
        ]
    }

    async fn handle(
        &self,
        method: &str,
        params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        match method {
            "list" => self.handle_list(params).await,
            "call" => self.handle_call(params).await,
            "get_schema" => self.handle_get_schema(params).await,
            "providers" => self.handle_providers(params).await,
            _ => Err(AdiServiceError::method_not_found(method)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_tools_service_list() {
        let service = ToolsService::new();

        let result = service.handle("list", json!({})).await.unwrap();
        match result {
            AdiHandleResult::Success(data) => {
                let tools = data.as_array().unwrap();
                assert!(!tools.is_empty());
                // Should have at least shell_execute and file_read
                let names: Vec<&str> = tools
                    .iter()
                    .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
                    .collect();
                assert!(names.contains(&"shell_execute"));
                assert!(names.contains(&"file_read"));
            }
            _ => panic!("Expected success"),
        }
    }

    #[tokio::test]
    async fn test_tools_service_call_shell() {
        let service = ToolsService::new();

        let result = service
            .handle(
                "call",
                json!({
                    "name": "shell_execute",
                    "arguments": {
                        "command": "echo 'hello world'"
                    }
                }),
            )
            .await
            .unwrap();

        match result {
            AdiHandleResult::Success(data) => {
                let content = data.get("content").and_then(|c| c.as_str()).unwrap();
                let parsed: JsonValue = serde_json::from_str(content).unwrap();
                assert!(parsed.get("success").and_then(|s| s.as_bool()).unwrap());
                assert!(parsed
                    .get("stdout")
                    .and_then(|s| s.as_str())
                    .unwrap()
                    .contains("hello world"));
            }
            _ => panic!("Expected success"),
        }
    }

    #[tokio::test]
    async fn test_tools_service_get_schema() {
        let service = ToolsService::new();

        let result = service
            .handle("get_schema", json!({"name": "shell_execute"}))
            .await
            .unwrap();

        match result {
            AdiHandleResult::Success(data) => {
                assert_eq!(data.get("name").and_then(|n| n.as_str()).unwrap(), "shell_execute");
                assert!(data.get("input_schema").is_some());
            }
            _ => panic!("Expected success"),
        }
    }

    #[tokio::test]
    async fn test_tools_service_providers() {
        let service = ToolsService::new();

        let result = service.handle("providers", json!({})).await.unwrap();
        match result {
            AdiHandleResult::Success(data) => {
                let providers = data.as_array().unwrap();
                assert!(providers.len() >= 2); // shell + filesystem
            }
            _ => panic!("Expected success"),
        }
    }

    #[tokio::test]
    async fn test_tools_service_tool_not_found() {
        let service = ToolsService::new();

        let result = service
            .handle(
                "call",
                json!({
                    "name": "nonexistent_tool",
                    "arguments": {}
                }),
            )
            .await;

        match result {
            Err(e) => {
                assert_eq!(e.code, "not_found");
            }
            _ => panic!("Expected not found error"),
        }
    }
}
