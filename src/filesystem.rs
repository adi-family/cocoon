//! File system operations for cocoon
//!
//! Provides file system browsing and reading capabilities over WebRTC data channels.
//! Supports listing directories, reading files, getting file stats, and walking directory trees.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::SystemTime;
use tokio::fs;
use walkdir::WalkDir;

// =============================================================================
// Request Types
// =============================================================================

/// File system request messages (from web client)
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileSystemRequest {
    /// List directory contents
    FsListDir {
        request_id: String,
        path: String,
    },

    /// Read file content
    FsReadFile {
        request_id: String,
        path: String,
        #[serde(default)]
        offset: Option<u64>,
        #[serde(default)]
        limit: Option<u64>,
    },

    /// Get file/directory stats
    FsStat {
        request_id: String,
        path: String,
    },

    /// Walk directory tree recursively
    FsWalk {
        request_id: String,
        path: String,
        #[serde(default)]
        max_depth: Option<usize>,
        #[serde(default)]
        pattern: Option<String>,
    },
}

// =============================================================================
// Response Types
// =============================================================================

/// File system response messages (to web client)
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileSystemResponse {
    /// Directory listing result
    FsDirListing {
        request_id: String,
        path: String,
        entries: Vec<FileEntry>,
    },

    /// File content result
    FsFileContent {
        request_id: String,
        path: String,
        content: String,
        encoding: String, // "utf8" or "base64"
        total_size: u64,
    },

    /// File stat result
    FsFileStat {
        request_id: String,
        path: String,
        stat: FileStat,
    },

    /// Directory walk result
    FsWalkResult {
        request_id: String,
        path: String,
        entries: Vec<WalkEntry>,
        truncated: bool,
    },

    /// Error response
    FsError {
        request_id: String,
        code: String,
        message: String,
    },
}

/// Directory entry
#[derive(Debug, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
}

/// File statistics
#[derive(Debug, Serialize)]
pub struct FileStat {
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<u32>,
}

/// Walk entry (for recursive directory listing)
#[derive(Debug, Serialize)]
pub struct WalkEntry {
    pub path: String,
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Convert SystemTime to ISO 8601 string
fn system_time_to_string(time: SystemTime) -> Option<String> {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| {
            let secs = d.as_secs();
            // Simple ISO 8601 format
            chrono::DateTime::from_timestamp(secs as i64, 0)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| format!("{}", secs))
        })
}

/// Check if content is likely binary
fn is_binary_content(data: &[u8]) -> bool {
    // Check first 8KB for null bytes
    let check_len = std::cmp::min(data.len(), 8192);
    data[..check_len].contains(&0)
}

/// Common text file extensions
const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "json", "yaml", "yml", "toml", "xml", "html", "htm",
    "css", "scss", "sass", "less", "js", "jsx", "ts", "tsx", "mjs", "cjs",
    "py", "rb", "rs", "go", "java", "c", "cpp", "cc", "h", "hpp", "cs",
    "php", "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd",
    "sql", "graphql", "gql", "vue", "svelte", "astro",
    "env", "gitignore", "dockerignore", "editorconfig",
    "makefile", "cmake", "dockerfile", "vagrantfile",
    "lock", "log", "csv", "tsv", "ini", "cfg", "conf", "config",
];

/// Check if file is likely a text file based on extension
fn is_text_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| TEXT_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

// =============================================================================
// File System Operations
// =============================================================================

/// Handle file system request and return response
pub async fn handle_request(request: FileSystemRequest) -> FileSystemResponse {
    match request {
        FileSystemRequest::FsListDir { request_id, path } => {
            list_directory(&request_id, &path).await
        }
        FileSystemRequest::FsReadFile {
            request_id,
            path,
            offset,
            limit,
        } => read_file(&request_id, &path, offset, limit).await,
        FileSystemRequest::FsStat { request_id, path } => {
            get_stat(&request_id, &path).await
        }
        FileSystemRequest::FsWalk {
            request_id,
            path,
            max_depth,
            pattern,
        } => walk_directory(&request_id, &path, max_depth, pattern).await,
    }
}

/// List directory contents
async fn list_directory(request_id: &str, path: &str) -> FileSystemResponse {
    let dir_path = Path::new(path);

    // Security check: ensure path doesn't escape allowed directories
    // For now, we allow all paths but log them
    tracing::debug!("Listing directory: {}", path);

    let mut entries = Vec::new();

    match fs::read_dir(dir_path).await {
        Ok(mut dir) => {
            while let Ok(Some(entry)) = dir.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                
                // Skip hidden files starting with . (optional, can be configurable)
                // For now, include all files
                
                match entry.metadata().await {
                    Ok(metadata) => {
                        let file_type = entry.file_type().await.ok();
                        let is_symlink = file_type.map(|ft| ft.is_symlink()).unwrap_or(false);
                        
                        entries.push(FileEntry {
                            name,
                            is_dir: metadata.is_dir(),
                            is_file: metadata.is_file(),
                            is_symlink,
                            size: if metadata.is_file() {
                                Some(metadata.len())
                            } else {
                                None
                            },
                            modified: metadata.modified().ok().and_then(system_time_to_string),
                        });
                    }
                    Err(e) => {
                        tracing::warn!("Failed to get metadata for {}: {}", name, e);
                        // Still include the entry with minimal info
                        entries.push(FileEntry {
                            name,
                            is_dir: false,
                            is_file: true,
                            is_symlink: false,
                            size: None,
                            modified: None,
                        });
                    }
                }
            }

            // Sort: directories first, then alphabetically
            entries.sort_by(|a, b| {
                match (a.is_dir, b.is_dir) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                }
            });

            FileSystemResponse::FsDirListing {
                request_id: request_id.to_string(),
                path: path.to_string(),
                entries,
            }
        }
        Err(e) => {
            tracing::error!("Failed to read directory {}: {}", path, e);
            FileSystemResponse::FsError {
                request_id: request_id.to_string(),
                code: error_code(&e),
                message: e.to_string(),
            }
        }
    }
}

/// Read file content
async fn read_file(
    request_id: &str,
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
) -> FileSystemResponse {
    let file_path = Path::new(path);
    
    tracing::debug!("Reading file: {} (offset: {:?}, limit: {:?})", path, offset, limit);

    // Get file metadata first
    let metadata = match fs::metadata(file_path).await {
        Ok(m) => m,
        Err(e) => {
            return FileSystemResponse::FsError {
                request_id: request_id.to_string(),
                code: error_code(&e),
                message: e.to_string(),
            };
        }
    };

    if !metadata.is_file() {
        return FileSystemResponse::FsError {
            request_id: request_id.to_string(),
            code: "not_a_file".to_string(),
            message: "Path is not a file".to_string(),
        };
    }

    let total_size = metadata.len();
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(1024 * 1024); // Default 1MB limit

    // Read file content
    match fs::read(file_path).await {
        Ok(content) => {
            // Apply offset and limit
            let start = std::cmp::min(offset as usize, content.len());
            let end = std::cmp::min(start + limit as usize, content.len());
            let slice = &content[start..end];

            // Determine if content should be base64 encoded
            let (encoded_content, encoding) = if is_binary_content(slice) || !is_text_file(file_path) {
                // Binary content - use base64
                (base64::Engine::encode(&base64::engine::general_purpose::STANDARD, slice), "base64".to_string())
            } else {
                // Text content - try UTF-8
                match String::from_utf8(slice.to_vec()) {
                    Ok(text) => (text, "utf8".to_string()),
                    Err(_) => {
                        // Fallback to base64 if not valid UTF-8
                        (base64::Engine::encode(&base64::engine::general_purpose::STANDARD, slice), "base64".to_string())
                    }
                }
            };

            FileSystemResponse::FsFileContent {
                request_id: request_id.to_string(),
                path: path.to_string(),
                content: encoded_content,
                encoding,
                total_size,
            }
        }
        Err(e) => {
            tracing::error!("Failed to read file {}: {}", path, e);
            FileSystemResponse::FsError {
                request_id: request_id.to_string(),
                code: error_code(&e),
                message: e.to_string(),
            }
        }
    }
}

/// Get file/directory statistics
async fn get_stat(request_id: &str, path: &str) -> FileSystemResponse {
    let file_path = Path::new(path);
    
    tracing::debug!("Getting stat for: {}", path);

    match fs::symlink_metadata(file_path).await {
        Ok(metadata) => {
            #[cfg(unix)]
            let permissions = {
                use std::os::unix::fs::PermissionsExt;
                Some(metadata.permissions().mode())
            };
            #[cfg(not(unix))]
            let permissions = None;

            FileSystemResponse::FsFileStat {
                request_id: request_id.to_string(),
                path: path.to_string(),
                stat: FileStat {
                    is_dir: metadata.is_dir(),
                    is_file: metadata.is_file(),
                    is_symlink: metadata.file_type().is_symlink(),
                    size: metadata.len(),
                    modified: metadata.modified().ok().and_then(system_time_to_string),
                    created: metadata.created().ok().and_then(system_time_to_string),
                    permissions,
                },
            }
        }
        Err(e) => {
            tracing::error!("Failed to stat {}: {}", path, e);
            FileSystemResponse::FsError {
                request_id: request_id.to_string(),
                code: error_code(&e),
                message: e.to_string(),
            }
        }
    }
}

/// Walk directory tree recursively
async fn walk_directory(
    request_id: &str,
    path: &str,
    max_depth: Option<usize>,
    pattern: Option<String>,
) -> FileSystemResponse {
    let dir_path = Path::new(path);
    
    tracing::debug!("Walking directory: {} (max_depth: {:?}, pattern: {:?})", path, max_depth, pattern);

    let max_depth = max_depth.unwrap_or(10);
    let max_entries = 10000; // Limit to prevent memory issues

    let mut entries = Vec::new();
    let mut truncated = false;

    // Compile glob pattern if provided
    let glob_pattern = pattern.as_ref().and_then(|p| glob::Pattern::new(p).ok());

    let walker = WalkDir::new(dir_path)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Skip hidden directories (starting with .)
            !e.file_name()
                .to_str()
                .map(|s| s.starts_with('.'))
                .unwrap_or(false)
        });

    for entry in walker {
        if entries.len() >= max_entries {
            truncated = true;
            break;
        }

        match entry {
            Ok(e) => {
                let entry_path = e.path().to_string_lossy().to_string();
                
                // Skip the root directory itself
                if entry_path == path {
                    continue;
                }

                // Apply pattern filter if provided
                if let Some(ref pattern) = glob_pattern {
                    let name = e.file_name().to_string_lossy();
                    if !pattern.matches(&name) {
                        continue;
                    }
                }

                let is_dir = e.file_type().is_dir();
                let size = if !is_dir {
                    e.metadata().ok().map(|m| m.len())
                } else {
                    None
                };

                entries.push(WalkEntry {
                    path: entry_path,
                    is_dir,
                    size,
                });
            }
            Err(e) => {
                tracing::warn!("Error walking directory: {}", e);
            }
        }
    }

    FileSystemResponse::FsWalkResult {
        request_id: request_id.to_string(),
        path: path.to_string(),
        entries,
        truncated,
    }
}

/// Convert IO error to error code
fn error_code(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => "not_found".to_string(),
        std::io::ErrorKind::PermissionDenied => "permission_denied".to_string(),
        std::io::ErrorKind::AlreadyExists => "already_exists".to_string(),
        std::io::ErrorKind::InvalidInput => "invalid_input".to_string(),
        std::io::ErrorKind::InvalidData => "invalid_data".to_string(),
        _ => "io_error".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::fs::File;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_list_directory() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        // Create some test files
        File::create(dir_path.join("file1.txt")).await.unwrap();
        File::create(dir_path.join("file2.rs")).await.unwrap();
        fs::create_dir(dir_path.join("subdir")).await.unwrap();

        let request = FileSystemRequest::FsListDir {
            request_id: "test-1".to_string(),
            path: dir_path.to_string_lossy().to_string(),
        };

        let response = handle_request(request).await;

        match response {
            FileSystemResponse::FsDirListing { entries, .. } => {
                assert_eq!(entries.len(), 3);
                // subdir should be first (directories first)
                assert!(entries[0].is_dir);
                assert_eq!(entries[0].name, "subdir");
            }
            _ => panic!("Expected FsDirListing response"),
        }
    }

    #[tokio::test]
    async fn test_read_text_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        
        let content = "Hello, World!";
        let mut file = File::create(&file_path).await.unwrap();
        file.write_all(content.as_bytes()).await.unwrap();

        let request = FileSystemRequest::FsReadFile {
            request_id: "test-2".to_string(),
            path: file_path.to_string_lossy().to_string(),
            offset: None,
            limit: None,
        };

        let response = handle_request(request).await;

        match response {
            FileSystemResponse::FsFileContent { content: read_content, encoding, .. } => {
                assert_eq!(encoding, "utf8");
                assert_eq!(read_content, content);
            }
            _ => panic!("Expected FsFileContent response"),
        }
    }

    #[tokio::test]
    async fn test_stat_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        
        let mut file = File::create(&file_path).await.unwrap();
        file.write_all(b"test content").await.unwrap();

        let request = FileSystemRequest::FsStat {
            request_id: "test-3".to_string(),
            path: file_path.to_string_lossy().to_string(),
        };

        let response = handle_request(request).await;

        match response {
            FileSystemResponse::FsFileStat { stat, .. } => {
                assert!(stat.is_file);
                assert!(!stat.is_dir);
                assert_eq!(stat.size, 12); // "test content" = 12 bytes
            }
            _ => panic!("Expected FsFileStat response"),
        }
    }

    #[tokio::test]
    async fn test_not_found_error() {
        let request = FileSystemRequest::FsListDir {
            request_id: "test-4".to_string(),
            path: "/nonexistent/path/that/does/not/exist".to_string(),
        };

        let response = handle_request(request).await;

        match response {
            FileSystemResponse::FsError { code, .. } => {
                assert_eq!(code, "not_found");
            }
            _ => panic!("Expected FsError response"),
        }
    }
}
