//! ADI Service implementations for cocoon
//!
//! This module contains service implementations that can be registered
//! with the AdiRouter to handle requests from the web app.

#[cfg(feature = "tasks-core")]
pub mod tasks;

pub mod tools;

#[cfg(feature = "tasks-core")]
pub use tasks::TasksService;

pub use tools::{
    FileSystemToolProvider, McpServerProvider, ShellToolProvider, ToolCategory, ToolContentType,
    ToolDef, ToolProvider, ToolResult, ToolsService,
};
