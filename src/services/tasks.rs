//! Tasks Service - ADI service implementation for task management
//!
//! Provides task CRUD operations, dependency management, and search
//! functionality via the ADI service protocol.
//!
//! ## MCP-Style Features
//!
//! - Full JSON Schema for all methods (params and results)
//! - Service capabilities declaration
//! - Subscription support for real-time task events
//! - Notifications for task lifecycle events

use crate::adi_router::{
    AdiHandleResult, AdiService, AdiServiceError, SubscriptionEvent, SubscriptionEventInfo,
};
use adi_tasks_core::{CreateTask, Task, TaskId, TaskManager, TaskStatus};
use async_trait::async_trait;
use lib_signaling_protocol::{AdiMethodInfo, AdiServiceCapabilities};
use serde_json::{json, Value as JsonValue};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

/// Tasks service for ADI router
///
/// Wraps TaskManager and provides it as an ADI service with full
/// MCP-style capabilities including subscriptions and notifications.
pub struct TasksService {
    manager: Arc<Mutex<TaskManager>>,
    /// Broadcast channel for task events
    event_tx: broadcast::Sender<SubscriptionEvent>,
}

impl TasksService {
    /// Create a new tasks service for a specific project
    pub fn new(project_path: &Path) -> Result<Self, String> {
        let manager = TaskManager::open(project_path)
            .map_err(|e| format!("Failed to open task manager: {}", e))?;
        let (event_tx, _) = broadcast::channel(256);
        Ok(Self {
            manager: Arc::new(Mutex::new(manager)),
            event_tx,
        })
    }

    /// Create a new tasks service using global task storage
    pub fn new_global() -> Result<Self, String> {
        let manager = TaskManager::open_global()
            .map_err(|e| format!("Failed to open global task manager: {}", e))?;
        let (event_tx, _) = broadcast::channel(256);
        Ok(Self {
            manager: Arc::new(Mutex::new(manager)),
            event_tx,
        })
    }

    /// Broadcast a task event to all subscribers
    fn broadcast_event(&self, event: &str, data: JsonValue) {
        let _ = self.event_tx.send(SubscriptionEvent {
            event: event.to_string(),
            data,
        });
    }

    /// Helper to convert Task to JSON with consistent schema
    fn task_to_json(task: &Task) -> JsonValue {
        json!({
            "id": task.id.0,
            "title": task.title,
            "description": task.description,
            "status": task.status.to_string(),
            "created_at": task.created_at,
            "updated_at": task.updated_at
        })
    }

    /// Handle list method
    async fn handle_list(&self, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let status_filter = params
            .get("status")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<TaskStatus>().ok());

        let manager = self.manager.lock().await;
        let tasks = if let Some(status) = status_filter {
            manager
                .get_by_status(status)
                .map_err(|e| AdiServiceError::internal(e.to_string()))?
        } else {
            manager
                .list()
                .map_err(|e| AdiServiceError::internal(e.to_string()))?
        };

        Ok(AdiHandleResult::Success(json!(tasks)))
    }

    /// Handle create method
    async fn handle_create(&self, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let title = params
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdiServiceError::invalid_params("title is required"))?;

        let description = params.get("description").and_then(|v| v.as_str());

        let depends_on: Vec<TaskId> = params
            .get("depends_on")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_i64().map(TaskId))
                    .collect()
            })
            .unwrap_or_default();

        let mut create_task = CreateTask::new(title);
        if let Some(desc) = description {
            create_task = create_task.with_description(desc);
        }
        create_task = create_task.with_dependencies(depends_on);

        let manager = self.manager.lock().await;
        let task_id = manager
            .create_task(create_task)
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        // Get the created task for the event
        if let Ok(task) = manager.get_task(task_id) {
            self.broadcast_event("task_created", Self::task_to_json(&task));
        }

        Ok(AdiHandleResult::Success(json!({ "task_id": task_id.0 })))
    }

    /// Handle get method
    async fn handle_get(&self, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let task_id = params
            .get("task_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| AdiServiceError::invalid_params("task_id is required"))?;

        let manager = self.manager.lock().await;
        let task_with_deps = manager
            .get_task_with_dependencies(TaskId(task_id))
            .map_err(|e| AdiServiceError::not_found(e.to_string()))?;

        Ok(AdiHandleResult::Success(json!(task_with_deps)))
    }

    /// Handle update method
    async fn handle_update(&self, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let task_id = params
            .get("task_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| AdiServiceError::invalid_params("task_id is required"))?;

        let manager = self.manager.lock().await;
        let mut task = manager
            .get_task(TaskId(task_id))
            .map_err(|e| AdiServiceError::not_found(e.to_string()))?;

        let old_status = task.status;

        // Update fields if provided
        if let Some(title) = params.get("title").and_then(|v| v.as_str()) {
            task.title = title.to_string();
        }
        if let Some(description) = params.get("description") {
            task.description = description.as_str().map(String::from);
        }
        if let Some(status) = params.get("status").and_then(|v| v.as_str()) {
            task.status = status
                .parse()
                .map_err(|_| AdiServiceError::invalid_params("invalid status"))?;
        }

        // Update timestamp
        task.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        manager
            .update_task(&task)
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        // Broadcast appropriate events
        self.broadcast_event("task_updated", Self::task_to_json(&task));
        
        // Broadcast status change if status changed
        if old_status != task.status {
            self.broadcast_event("task_status_changed", json!({
                "task_id": task_id,
                "old_status": old_status.to_string(),
                "new_status": task.status.to_string()
            }));
        }

        Ok(AdiHandleResult::Success(json!({ "task_id": task_id })))
    }

    /// Handle delete method
    async fn handle_delete(&self, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let task_id = params
            .get("task_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| AdiServiceError::invalid_params("task_id is required"))?;

        let manager = self.manager.lock().await;
        
        // Get task info before deleting for the event
        let task_info = manager.get_task(TaskId(task_id)).ok();
        
        manager
            .delete_task(TaskId(task_id))
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        // Broadcast delete event
        self.broadcast_event("task_deleted", json!({
            "task_id": task_id,
            "title": task_info.as_ref().map(|t| t.title.as_str())
        }));

        Ok(AdiHandleResult::Success(json!({ "deleted": true })))
    }

    /// Handle search method
    async fn handle_search(&self, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdiServiceError::invalid_params("query is required"))?;

        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(20) as usize;

        let manager = self.manager.lock().await;
        let tasks = manager
            .search(query, limit)
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        Ok(AdiHandleResult::Success(json!(tasks)))
    }

    /// Handle ready method (tasks with no incomplete dependencies)
    async fn handle_ready(&self, _params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let manager = self.manager.lock().await;
        let tasks = manager
            .get_ready()
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        Ok(AdiHandleResult::Success(json!(tasks)))
    }

    /// Handle blocked method
    async fn handle_blocked(&self, _params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let manager = self.manager.lock().await;
        let tasks = manager
            .get_blocked()
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        Ok(AdiHandleResult::Success(json!(tasks)))
    }

    /// Handle stats method
    async fn handle_stats(&self, _params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let manager = self.manager.lock().await;
        let status = manager
            .status()
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        Ok(AdiHandleResult::Success(json!(status)))
    }

    /// Handle add_dependency method
    async fn handle_add_dependency(
        &self,
        params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        let from_id = params
            .get("from_task_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| AdiServiceError::invalid_params("from_task_id is required"))?;

        let to_id = params
            .get("to_task_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| AdiServiceError::invalid_params("to_task_id is required"))?;

        let manager = self.manager.lock().await;
        manager
            .add_dependency(TaskId(from_id), TaskId(to_id))
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        Ok(AdiHandleResult::Success(
            json!({ "from_task_id": from_id, "to_task_id": to_id }),
        ))
    }

    /// Handle remove_dependency method
    async fn handle_remove_dependency(
        &self,
        params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        let from_id = params
            .get("from_task_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| AdiServiceError::invalid_params("from_task_id is required"))?;

        let to_id = params
            .get("to_task_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| AdiServiceError::invalid_params("to_task_id is required"))?;

        let manager = self.manager.lock().await;
        manager
            .remove_dependency(TaskId(from_id), TaskId(to_id))
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        Ok(AdiHandleResult::Success(json!({ "removed": true })))
    }

    /// Handle detect_cycles method
    async fn handle_detect_cycles(
        &self,
        _params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        let manager = self.manager.lock().await;
        let cycles = manager
            .detect_cycles()
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        // Convert Vec<Vec<TaskId>> to Vec<Vec<i64>> for JSON
        let cycles: Vec<Vec<i64>> = cycles.into_iter().map(|c| c.into_iter().map(|id| id.0).collect()).collect();

        Ok(AdiHandleResult::Success(json!({ "cycles": cycles })))
    }
}

#[async_trait]
impl AdiService for TasksService {
    // ========== Identity ==========

    fn service_id(&self) -> &str {
        "tasks"
    }

    fn name(&self) -> &str {
        "Task Management"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> Option<&str> {
        Some("Manage tasks with dependencies, status tracking, and full-text search. Supports real-time subscriptions for task lifecycle events.")
    }

    // ========== Capabilities ==========

    fn capabilities(&self) -> AdiServiceCapabilities {
        AdiServiceCapabilities {
            subscriptions: true,
            notifications: true,
            streaming: true,
        }
    }

    // ========== Methods ==========

    fn methods(&self) -> Vec<AdiMethodInfo> {
        vec![
            AdiMethodInfo {
                name: "list".to_string(),
                description: "List all tasks, optionally filtered by status".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "status": { 
                            "type": "string", 
                            "enum": ["todo", "in_progress", "done", "blocked", "cancelled"],
                            "description": "Filter tasks by status"
                        }
                    }
                })),
                result_schema: Some(json!({
                    "type": "array",
                    "items": { "$ref": "#/definitions/Task" },
                    "definitions": {
                        "Task": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "integer" },
                                "title": { "type": "string" },
                                "description": { "type": ["string", "null"] },
                                "status": { "type": "string", "enum": ["todo", "in_progress", "done", "blocked", "cancelled"] },
                                "created_at": { "type": "integer", "description": "Unix timestamp" },
                                "updated_at": { "type": "integer", "description": "Unix timestamp" }
                            }
                        }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "create".to_string(),
                description: "Create a new task. Emits 'task_created' event.".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["title"],
                    "properties": {
                        "title": { "type": "string", "description": "Task title" },
                        "description": { "type": "string", "description": "Optional task description" },
                        "depends_on": { 
                            "type": "array", 
                            "items": { "type": "integer" },
                            "description": "IDs of tasks this task depends on"
                        }
                    }
                })),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "integer", "description": "ID of the created task" }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "get".to_string(),
                description: "Get a task by ID with its dependencies".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["task_id"],
                    "properties": {
                        "task_id": { "type": "integer", "description": "Task ID to retrieve" }
                    }
                })),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "task": { "$ref": "#/definitions/Task" },
                        "depends_on": {
                            "type": "array",
                            "items": { "$ref": "#/definitions/Task" },
                            "description": "Tasks this task depends on"
                        },
                        "dependents": {
                            "type": "array",
                            "items": { "$ref": "#/definitions/Task" },
                            "description": "Tasks that depend on this task"
                        }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "update".to_string(),
                description: "Update task properties. Emits 'task_updated' and optionally 'task_status_changed' events.".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["task_id"],
                    "properties": {
                        "task_id": { "type": "integer", "description": "Task ID to update" },
                        "title": { "type": "string", "description": "New title" },
                        "description": { "type": "string", "description": "New description" },
                        "status": { 
                            "type": "string", 
                            "enum": ["todo", "in_progress", "done", "blocked", "cancelled"],
                            "description": "New status"
                        }
                    }
                })),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "integer" }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "delete".to_string(),
                description: "Delete a task. Emits 'task_deleted' event.".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["task_id"],
                    "properties": {
                        "task_id": { "type": "integer", "description": "Task ID to delete" }
                    }
                })),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "deleted": { "type": "boolean" }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "search".to_string(),
                description: "Search tasks using full-text search".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": { "type": "string", "description": "Search query" },
                        "limit": { "type": "integer", "default": 20, "description": "Max results to return" }
                    }
                })),
                result_schema: Some(json!({
                    "type": "array",
                    "items": { "$ref": "#/definitions/Task" }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "ready".to_string(),
                description: "Get tasks ready to work on (no incomplete dependencies)".to_string(),
                streaming: false,
                params_schema: Some(json!({ "type": "object" })),
                result_schema: Some(json!({
                    "type": "array",
                    "items": { "$ref": "#/definitions/Task" }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "blocked".to_string(),
                description: "Get tasks blocked by incomplete dependencies".to_string(),
                streaming: false,
                params_schema: Some(json!({ "type": "object" })),
                result_schema: Some(json!({
                    "type": "array",
                    "items": { "$ref": "#/definitions/Task" }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "stats".to_string(),
                description: "Get task statistics".to_string(),
                streaming: false,
                params_schema: Some(json!({ "type": "object" })),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "total_tasks": { "type": "integer" },
                        "todo_count": { "type": "integer" },
                        "in_progress_count": { "type": "integer" },
                        "done_count": { "type": "integer" },
                        "blocked_count": { "type": "integer" },
                        "cancelled_count": { "type": "integer" }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "add_dependency".to_string(),
                description: "Add a dependency between tasks".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["from_task_id", "to_task_id"],
                    "properties": {
                        "from_task_id": { "type": "integer", "description": "Task that depends on another" },
                        "to_task_id": { "type": "integer", "description": "Task that must be completed first" }
                    }
                })),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "from_task_id": { "type": "integer" },
                        "to_task_id": { "type": "integer" }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "remove_dependency".to_string(),
                description: "Remove a dependency between tasks".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["from_task_id", "to_task_id"],
                    "properties": {
                        "from_task_id": { "type": "integer" },
                        "to_task_id": { "type": "integer" }
                    }
                })),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "removed": { "type": "boolean" }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "detect_cycles".to_string(),
                description: "Detect dependency cycles in the task graph".to_string(),
                streaming: false,
                params_schema: Some(json!({ "type": "object" })),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "cycles": {
                            "type": "array",
                            "items": {
                                "type": "array",
                                "items": { "type": "integer" },
                                "description": "Task IDs forming a cycle"
                            }
                        }
                    }
                })),
                ..Default::default()
            },
        ]
    }

    // ========== Subscriptions ==========

    fn subscription_events(&self) -> Vec<SubscriptionEventInfo> {
        vec![
            SubscriptionEventInfo {
                name: "task_created".to_string(),
                description: "Emitted when a new task is created".to_string(),
                data_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "integer" },
                        "title": { "type": "string" },
                        "description": { "type": ["string", "null"] },
                        "status": { "type": "string" },
                        "created_at": { "type": "integer" },
                        "updated_at": { "type": "integer" }
                    }
                })),
            },
            SubscriptionEventInfo {
                name: "task_updated".to_string(),
                description: "Emitted when a task is updated".to_string(),
                data_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "integer" },
                        "title": { "type": "string" },
                        "description": { "type": ["string", "null"] },
                        "status": { "type": "string" },
                        "created_at": { "type": "integer" },
                        "updated_at": { "type": "integer" }
                    }
                })),
            },
            SubscriptionEventInfo {
                name: "task_deleted".to_string(),
                description: "Emitted when a task is deleted".to_string(),
                data_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "integer" },
                        "title": { "type": ["string", "null"] }
                    }
                })),
            },
            SubscriptionEventInfo {
                name: "task_status_changed".to_string(),
                description: "Emitted when a task's status changes".to_string(),
                data_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "integer" },
                        "old_status": { "type": "string" },
                        "new_status": { "type": "string" }
                    }
                })),
            },
            SubscriptionEventInfo {
                name: "*".to_string(),
                description: "Subscribe to all task events".to_string(),
                data_schema: None,
            },
        ]
    }

    async fn subscribe(
        &self,
        event: &str,
        _filter: Option<JsonValue>,
    ) -> Result<broadcast::Receiver<SubscriptionEvent>, AdiServiceError> {
        // Validate event name
        let valid_events = ["task_created", "task_updated", "task_deleted", "task_status_changed", "*"];
        if !valid_events.contains(&event) {
            return Err(AdiServiceError::invalid_params(format!(
                "Unknown event '{}'. Valid events: {:?}",
                event, valid_events
            )));
        }

        // Return a receiver for the broadcast channel
        // Note: filtering by event name would be done by the caller
        Ok(self.event_tx.subscribe())
    }

    // ========== Request Handling ==========

    async fn handle(
        &self,
        method: &str,
        params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        match method {
            "list" => self.handle_list(params).await,
            "create" => self.handle_create(params).await,
            "get" => self.handle_get(params).await,
            "update" => self.handle_update(params).await,
            "delete" => self.handle_delete(params).await,
            "search" => self.handle_search(params).await,
            "ready" => self.handle_ready(params).await,
            "blocked" => self.handle_blocked(params).await,
            "stats" => self.handle_stats(params).await,
            "add_dependency" => self.handle_add_dependency(params).await,
            "remove_dependency" => self.handle_remove_dependency(params).await,
            "detect_cycles" => self.handle_detect_cycles(params).await,
            _ => Err(AdiServiceError::method_not_found(method)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_tasks_service_create_and_list() {
        let dir = tempdir().unwrap();
        let service = TasksService::new(dir.path()).unwrap();

        // Create a task
        let result = service
            .handle("create", json!({"title": "Test task", "description": "A test"}))
            .await
            .unwrap();

        match result {
            AdiHandleResult::Success(data) => {
                assert!(data.get("task_id").is_some());
            }
            _ => panic!("Expected success"),
        }

        // List tasks
        let result = service.handle("list", json!({})).await.unwrap();
        match result {
            AdiHandleResult::Success(data) => {
                let tasks = data.as_array().unwrap();
                assert_eq!(tasks.len(), 1);
                assert_eq!(tasks[0]["title"], "Test task");
            }
            _ => panic!("Expected success"),
        }
    }

    #[test]
    fn test_tasks_service_metadata() {
        let dir = tempdir().unwrap();
        let service = TasksService::new(dir.path()).unwrap();

        assert_eq!(service.service_id(), "tasks");
        assert_eq!(service.name(), "Task Management");
        assert!(service.description().is_some());
        
        let caps = service.capabilities();
        assert!(caps.subscriptions);
        assert!(caps.notifications);
        assert!(caps.streaming);
    }

    #[test]
    fn test_tasks_service_methods_have_schemas() {
        let dir = tempdir().unwrap();
        let service = TasksService::new(dir.path()).unwrap();

        let methods = service.methods();
        assert!(!methods.is_empty());

        // All methods should have result_schema
        for method in &methods {
            assert!(
                method.result_schema.is_some(),
                "Method '{}' should have result_schema",
                method.name
            );
        }

        // create, get, update, delete, search should have params_schema
        let methods_with_params = ["create", "get", "update", "delete", "search", "add_dependency", "remove_dependency"];
        for method in &methods {
            if methods_with_params.contains(&method.name.as_str()) {
                assert!(
                    method.params_schema.is_some(),
                    "Method '{}' should have params_schema",
                    method.name
                );
            }
        }
    }

    #[test]
    fn test_tasks_service_subscription_events() {
        let dir = tempdir().unwrap();
        let service = TasksService::new(dir.path()).unwrap();

        let events = service.subscription_events();
        assert!(!events.is_empty());

        let event_names: Vec<_> = events.iter().map(|e| e.name.as_str()).collect();
        assert!(event_names.contains(&"task_created"));
        assert!(event_names.contains(&"task_updated"));
        assert!(event_names.contains(&"task_deleted"));
        assert!(event_names.contains(&"task_status_changed"));
        assert!(event_names.contains(&"*"));
    }

    #[tokio::test]
    async fn test_tasks_service_subscription() {
        let dir = tempdir().unwrap();
        let service = TasksService::new(dir.path()).unwrap();

        // Subscribe to task events
        let mut receiver = service.subscribe("task_created", None).await.unwrap();

        // Create a task - should emit event
        let _ = service
            .handle("create", json!({"title": "Subscribed task"}))
            .await
            .unwrap();

        // Should receive the event
        let event = receiver.try_recv().unwrap();
        assert_eq!(event.event, "task_created");
        assert_eq!(event.data["title"], "Subscribed task");
    }

    #[tokio::test]
    async fn test_tasks_service_status_change_event() {
        let dir = tempdir().unwrap();
        let service = TasksService::new(dir.path()).unwrap();

        // Subscribe to events
        let mut receiver = service.subscribe("*", None).await.unwrap();

        // Create a task
        let result = service
            .handle("create", json!({"title": "Status test"}))
            .await
            .unwrap();
        
        let task_id = match result {
            AdiHandleResult::Success(data) => data["task_id"].as_i64().unwrap(),
            _ => panic!("Expected success"),
        };

        // Consume the create event
        let _ = receiver.try_recv().unwrap();

        // Update status
        let _ = service
            .handle("update", json!({"task_id": task_id, "status": "in_progress"}))
            .await
            .unwrap();

        // Should receive task_updated event
        let event = receiver.try_recv().unwrap();
        assert_eq!(event.event, "task_updated");

        // Should also receive task_status_changed event
        let event = receiver.try_recv().unwrap();
        assert_eq!(event.event, "task_status_changed");
        assert_eq!(event.data["old_status"], "todo");
        assert_eq!(event.data["new_status"], "in_progress");
    }

    #[tokio::test]
    async fn test_tasks_service_dependencies() {
        let dir = tempdir().unwrap();
        let service = TasksService::new(dir.path()).unwrap();

        // Create two tasks
        let r1 = service
            .handle("create", json!({"title": "Task 1"}))
            .await
            .unwrap();
        let r2 = service
            .handle("create", json!({"title": "Task 2"}))
            .await
            .unwrap();

        let id1 = match r1 {
            AdiHandleResult::Success(d) => d["task_id"].as_i64().unwrap(),
            _ => panic!("Expected success"),
        };
        let id2 = match r2 {
            AdiHandleResult::Success(d) => d["task_id"].as_i64().unwrap(),
            _ => panic!("Expected success"),
        };

        // Add dependency: Task 2 depends on Task 1
        let result = service
            .handle(
                "add_dependency",
                json!({"from_task_id": id2, "to_task_id": id1}),
            )
            .await
            .unwrap();

        match result {
            AdiHandleResult::Success(_) => {}
            _ => panic!("Expected success"),
        }

        // Get task 2 with dependencies
        let result = service
            .handle("get", json!({"task_id": id2}))
            .await
            .unwrap();

        match result {
            AdiHandleResult::Success(data) => {
                let depends_on = data["depends_on"].as_array().unwrap();
                assert_eq!(depends_on.len(), 1);
                assert_eq!(depends_on[0]["title"], "Task 1");
            }
            _ => panic!("Expected success"),
        }
    }

    #[tokio::test]
    async fn test_tasks_service_stats() {
        let dir = tempdir().unwrap();
        let service = TasksService::new(dir.path()).unwrap();

        // Create tasks
        service
            .handle("create", json!({"title": "Task 1"}))
            .await
            .unwrap();
        service
            .handle("create", json!({"title": "Task 2"}))
            .await
            .unwrap();

        // Get stats
        let result = service.handle("stats", json!({})).await.unwrap();
        match result {
            AdiHandleResult::Success(data) => {
                assert_eq!(data["total_tasks"], 2);
                assert_eq!(data["todo_count"], 2);
            }
            _ => panic!("Expected success"),
        }
    }
}
