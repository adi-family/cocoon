//! ADI Service implementations for cocoon
//!
//! This module contains service implementations that can be registered
//! with the AdiRouter to handle requests from the web app.

#[cfg(feature = "tasks-core")]
pub mod tasks;

#[cfg(feature = "kb-core")]
pub mod knowledgebase;

pub mod plugin;
pub mod tools;

#[cfg(feature = "credentials-core")]
pub mod credentials;

#[cfg(feature = "tasks-core")]
pub use tasks::TasksService;

#[cfg(feature = "kb-core")]
pub use knowledgebase::KnowledgebaseService;

#[cfg(feature = "credentials-core")]
pub use credentials::{CredentialsService, CredentialsServiceAdi};

pub use tools::{
    FileSystemToolProvider, McpServerProvider, ShellToolProvider, ToolCategory, ToolContentType,
    ToolDef, ToolProvider, ToolResult, ToolsService,
};
