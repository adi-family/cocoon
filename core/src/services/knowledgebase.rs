//! Knowledgebase Service - ADI service implementation for knowledge management
//!
//! Provides semantic search, graph DB operations, and knowledge management
//! via the ADI service protocol.

use crate::adi_router::{
    AdiHandleResult, AdiService, AdiServiceError, SubscriptionEvent, SubscriptionEventInfo,
};
use async_trait::async_trait;
use kb_core::{EdgeType, Knowledgebase, NodeType};
use crate::protocol::types::{AdiMethodInfo, AdiServiceCapabilities};
use serde_json::{json, Value as JsonValue};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use uuid::Uuid;

/// Knowledgebase service for ADI router
///
/// Wraps knowledgebase-core and exposes semantic search, node CRUD,
/// edge management, and conflict detection as an ADI service.
pub struct KnowledgebaseService {
    kb: Arc<Mutex<Knowledgebase>>,
    event_tx: broadcast::Sender<SubscriptionEvent>,
}

impl KnowledgebaseService {
    /// Create a new knowledgebase service using default data directory with fastembed
    pub async fn new() -> Result<Self, String> {
        let data_dir = kb_core::default_data_dir();
        #[allow(deprecated)]
        let kb = Knowledgebase::open(&data_dir)
            .await
            .map_err(|e| format!("Failed to open knowledgebase: {}", e))?;
        let (event_tx, _) = broadcast::channel(256);
        Ok(Self {
            kb: Arc::new(Mutex::new(kb)),
            event_tx,
        })
    }

    fn broadcast_event(&self, event: &str, data: JsonValue) {
        let _ = self.event_tx.send(SubscriptionEvent {
            event: event.to_string(),
            data,
        });
    }

    fn parse_node_type(s: Option<&str>) -> NodeType {
        match s {
            Some("decision") => NodeType::Decision,
            Some("fact") => NodeType::Fact,
            Some("error") => NodeType::Error,
            Some("guide") => NodeType::Guide,
            Some("glossary") => NodeType::Glossary,
            Some("context") => NodeType::Context,
            Some("assumption") => NodeType::Assumption,
            _ => NodeType::Fact,
        }
    }

    fn parse_edge_type(s: Option<&str>) -> EdgeType {
        match s {
            Some("supersedes") => EdgeType::Supersedes,
            Some("contradicts") => EdgeType::Contradicts,
            Some("requires") => EdgeType::Requires,
            Some("related_to") => EdgeType::RelatedTo,
            Some("derived_from") => EdgeType::DerivedFrom,
            Some("answers") => EdgeType::Answers,
            _ => EdgeType::RelatedTo,
        }
    }

    async fn handle_query(&self, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let q = params
            .get("q")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdiServiceError::invalid_params("q is required"))?;

        let kb = self.kb.lock().await;
        let results = kb
            .query(q)
            .await
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        Ok(AdiHandleResult::Success(json!(results)))
    }

    async fn handle_subgraph(
        &self,
        params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        let q = params
            .get("q")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdiServiceError::invalid_params("q is required"))?;

        let kb = self.kb.lock().await;
        let subgraph = kb
            .query_subgraph(q)
            .await
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        Ok(AdiHandleResult::Success(json!(subgraph)))
    }

    async fn handle_add(&self, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let user_said = params
            .get("user_said")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdiServiceError::invalid_params("user_said is required"))?;

        let derived_knowledge = params
            .get("derived_knowledge")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AdiServiceError::invalid_params("derived_knowledge is required"))?;

        let node_type =
            Self::parse_node_type(params.get("node_type").and_then(|v| v.as_str()));

        let kb = self.kb.lock().await;
        let node = kb
            .add_from_user(user_said, derived_knowledge, node_type)
            .await
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        self.broadcast_event("node_added", json!({ "id": node.id, "title": node.title }));

        Ok(AdiHandleResult::Success(json!(node)))
    }

    async fn handle_get(&self, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| AdiServiceError::invalid_params("id (uuid) is required"))?;

        let kb = self.kb.lock().await;
        let node = kb
            .get_node(id)
            .map_err(|e| AdiServiceError::internal(e.to_string()))?
            .ok_or_else(|| AdiServiceError::not_found(format!("Node {} not found", id)))?;

        Ok(AdiHandleResult::Success(json!(node)))
    }

    async fn handle_delete(&self, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| AdiServiceError::invalid_params("id (uuid) is required"))?;

        let kb = self.kb.lock().await;
        kb.delete_node(id)
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        self.broadcast_event("node_deleted", json!({ "id": id.to_string() }));

        Ok(AdiHandleResult::Success(json!({ "deleted": true })))
    }

    async fn handle_approve(&self, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| AdiServiceError::invalid_params("id (uuid) is required"))?;

        let kb = self.kb.lock().await;
        kb.approve(id)
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        self.broadcast_event("node_approved", json!({ "id": id.to_string() }));

        Ok(AdiHandleResult::Success(json!({ "approved": true })))
    }

    async fn handle_conflicts(
        &self,
        _params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        let kb = self.kb.lock().await;
        let conflicts = kb
            .get_conflicts()
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        let pairs: Vec<JsonValue> = conflicts
            .into_iter()
            .map(|(a, b)| json!({ "node_a": a, "node_b": b }))
            .collect();

        Ok(AdiHandleResult::Success(json!(pairs)))
    }

    async fn handle_orphans(
        &self,
        _params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        let kb = self.kb.lock().await;
        let orphans = kb
            .get_orphans()
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        Ok(AdiHandleResult::Success(json!(orphans)))
    }

    async fn handle_link(&self, params: JsonValue) -> Result<AdiHandleResult, AdiServiceError> {
        let from_id = params
            .get("from_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| AdiServiceError::invalid_params("from_id (uuid) is required"))?;

        let to_id = params
            .get("to_id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| AdiServiceError::invalid_params("to_id (uuid) is required"))?;

        let edge_type =
            Self::parse_edge_type(params.get("edge_type").and_then(|v| v.as_str()));

        let weight = params
            .get("weight")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as f32;

        let kb = self.kb.lock().await;
        let edge = kb
            .add_edge(from_id, to_id, edge_type, weight)
            .map_err(|e| AdiServiceError::internal(e.to_string()))?;

        self.broadcast_event("edge_added", json!({ "id": edge.id, "from_id": from_id.to_string(), "to_id": to_id.to_string() }));

        Ok(AdiHandleResult::Success(json!(edge)))
    }

    async fn handle_status(
        &self,
        _params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        let kb = self.kb.lock().await;
        let data_dir = kb.data_dir().to_string_lossy().to_string();
        let embedding_count = kb.storage().embedding.count();

        Ok(AdiHandleResult::Success(json!({
            "initialized": true,
            "data_dir": data_dir,
            "embedding_count": embedding_count,
        })))
    }
}

#[async_trait]
impl AdiService for KnowledgebaseService {
    fn service_id(&self) -> &str {
        "kb"
    }

    fn name(&self) -> &str {
        "Knowledgebase"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> Option<&str> {
        Some("Semantic search, graph DB, and knowledge management. Store, query, and connect knowledge nodes with confidence scoring and conflict detection.")
    }

    fn capabilities(&self) -> AdiServiceCapabilities {
        AdiServiceCapabilities {
            subscriptions: true,
            notifications: true,
            streaming: false,
        }
    }

    fn methods(&self) -> Vec<AdiMethodInfo> {
        vec![
            AdiMethodInfo {
                name: "query".to_string(),
                description: "Semantic search across knowledge nodes".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["q"],
                    "properties": {
                        "q": { "type": "string", "description": "Search query" },
                        "limit": { "type": "integer", "description": "Max results" }
                    }
                })),
                result_schema: Some(json!({
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "node": { "$ref": "#/definitions/Node" },
                            "score": { "type": "number" },
                            "edges": { "type": "array", "items": { "$ref": "#/definitions/Edge" } }
                        }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "subgraph".to_string(),
                description: "Get subgraph for agent consumption".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["q"],
                    "properties": {
                        "q": { "type": "string", "description": "Search query" }
                    }
                })),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "nodes": { "type": "array", "items": { "$ref": "#/definitions/Node" } },
                        "edges": { "type": "array", "items": { "$ref": "#/definitions/Edge" } }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "add".to_string(),
                description: "Add knowledge from user statement. Emits 'node_added' event.".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["user_said", "derived_knowledge"],
                    "properties": {
                        "user_said": { "type": "string", "description": "Original user statement" },
                        "derived_knowledge": { "type": "string", "description": "Derived knowledge content" },
                        "node_type": {
                            "type": "string",
                            "enum": ["decision", "fact", "error", "guide", "glossary", "context", "assumption"],
                            "description": "Type of knowledge node (default: fact)"
                        }
                    }
                })),
                result_schema: Some(json!({ "$ref": "#/definitions/Node" })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "get".to_string(),
                description: "Get a knowledge node by ID".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid", "description": "Node UUID" }
                    }
                })),
                result_schema: Some(json!({ "$ref": "#/definitions/Node" })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "delete".to_string(),
                description: "Delete a knowledge node. Emits 'node_deleted' event.".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid", "description": "Node UUID" }
                    }
                })),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": { "deleted": { "type": "boolean" } }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "approve".to_string(),
                description: "Approve a node (set confidence to 1.0). Emits 'node_approved' event.".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string", "format": "uuid", "description": "Node UUID" }
                    }
                })),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": { "approved": { "type": "boolean" } }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "conflicts".to_string(),
                description: "Get contradicting node pairs".to_string(),
                streaming: false,
                params_schema: Some(json!({ "type": "object" })),
                result_schema: Some(json!({
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "node_a": { "$ref": "#/definitions/Node" },
                            "node_b": { "$ref": "#/definitions/Node" }
                        }
                    }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "orphans".to_string(),
                description: "Get nodes with no edges".to_string(),
                streaming: false,
                params_schema: Some(json!({ "type": "object" })),
                result_schema: Some(json!({
                    "type": "array",
                    "items": { "$ref": "#/definitions/Node" }
                })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "link".to_string(),
                description: "Create an edge between nodes. Emits 'edge_added' event.".to_string(),
                streaming: false,
                params_schema: Some(json!({
                    "type": "object",
                    "required": ["from_id", "to_id"],
                    "properties": {
                        "from_id": { "type": "string", "format": "uuid", "description": "Source node UUID" },
                        "to_id": { "type": "string", "format": "uuid", "description": "Target node UUID" },
                        "edge_type": {
                            "type": "string",
                            "enum": ["supersedes", "contradicts", "requires", "related_to", "derived_from", "answers"],
                            "description": "Edge type (default: related_to)"
                        },
                        "weight": { "type": "number", "description": "Edge weight 0.0-1.0 (default: 1.0)" }
                    }
                })),
                result_schema: Some(json!({ "$ref": "#/definitions/Edge" })),
                ..Default::default()
            },
            AdiMethodInfo {
                name: "status".to_string(),
                description: "Get knowledgebase status".to_string(),
                streaming: false,
                params_schema: Some(json!({ "type": "object" })),
                result_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "initialized": { "type": "boolean" },
                        "data_dir": { "type": "string" },
                        "embedding_count": { "type": "integer" }
                    }
                })),
                ..Default::default()
            },
        ]
    }

    fn subscription_events(&self) -> Vec<SubscriptionEventInfo> {
        vec![
            SubscriptionEventInfo {
                name: "node_added".to_string(),
                description: "Emitted when a knowledge node is added".to_string(),
                data_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "title": { "type": "string" }
                    }
                })),
            },
            SubscriptionEventInfo {
                name: "node_deleted".to_string(),
                description: "Emitted when a knowledge node is deleted".to_string(),
                data_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" }
                    }
                })),
            },
            SubscriptionEventInfo {
                name: "node_approved".to_string(),
                description: "Emitted when a knowledge node is approved".to_string(),
                data_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" }
                    }
                })),
            },
            SubscriptionEventInfo {
                name: "edge_added".to_string(),
                description: "Emitted when an edge is created between nodes".to_string(),
                data_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "from_id": { "type": "string" },
                        "to_id": { "type": "string" }
                    }
                })),
            },
            SubscriptionEventInfo {
                name: "*".to_string(),
                description: "Subscribe to all knowledgebase events".to_string(),
                data_schema: None,
            },
        ]
    }

    async fn subscribe(
        &self,
        event: &str,
        _filter: Option<JsonValue>,
    ) -> Result<broadcast::Receiver<SubscriptionEvent>, AdiServiceError> {
        let valid_events = [
            "node_added",
            "node_deleted",
            "node_approved",
            "edge_added",
            "*",
        ];
        if !valid_events.contains(&event) {
            return Err(AdiServiceError::invalid_params(format!(
                "Unknown event '{}'. Valid events: {:?}",
                event, valid_events
            )));
        }

        Ok(self.event_tx.subscribe())
    }

    async fn handle(
        &self,
        method: &str,
        params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        match method {
            "query" => self.handle_query(params).await,
            "subgraph" => self.handle_subgraph(params).await,
            "add" => self.handle_add(params).await,
            "get" => self.handle_get(params).await,
            "delete" => self.handle_delete(params).await,
            "approve" => self.handle_approve(params).await,
            "conflicts" => self.handle_conflicts(params).await,
            "orphans" => self.handle_orphans(params).await,
            "link" => self.handle_link(params).await,
            "status" => self.handle_status(params).await,
            _ => Err(AdiServiceError::method_not_found(method)),
        }
    }
}
