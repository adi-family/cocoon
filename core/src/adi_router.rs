//! ADI Service Router
//!
//! Generic router for ADI services. Services register themselves with the router
//! and receive requests via the "adi" WebRTC data channel.
//!
//! Supports request/response, streaming, subscriptions, and notifications.
//!
//! ## MCP-Style Architecture
//!
//! Services implement the `AdiService` trait which provides:
//! - Service metadata (id, name, version, description)
//! - Method definitions with JSON Schema
//! - Service capabilities (subscriptions, notifications, streaming)
//! - Request handling
//! - Optional subscription support for real-time events

use async_trait::async_trait;
use crate::protocol::types::{AdiMethodInfo, AdiPluginCapabilities, AdiPluginInfo};
use serde::{Serialize, Deserialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdiRequest {
    pub request_id: Uuid,
    pub plugin: String,
    pub method: String,
    pub params: JsonValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdiResponse {
    Success { request_id: Uuid, plugin: String, method: String, data: JsonValue },
    Error { request_id: Uuid, plugin: String, method: String, code: String, message: String },
    PluginNotFound { request_id: Uuid, plugin: String },
    MethodNotFound { request_id: Uuid, plugin: String, method: String, available_methods: Vec<String> },
    Stream { request_id: Uuid, plugin: String, method: String, data: JsonValue, seq: u32, done: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdiDiscovery {
    ListPlugins { request_id: Uuid },
    PluginsList { request_id: Uuid, plugins: Vec<AdiPluginInfo> },
}

#[derive(Debug, Clone)]
pub enum AdiNotification {
    PluginsChanged { added: Vec<String>, removed: Vec<String>, updated: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdiSubscription {
    Subscribe { request_id: Uuid, plugin: String, event: String, filter: Option<JsonValue> },
    Subscribed { request_id: Uuid, subscription_id: Uuid, plugin: String, event: String },
    Unsubscribe { subscription_id: Uuid },
    Unsubscribed { subscription_id: Uuid },
    Error { request_id: Uuid, code: String, message: String },
}

/// Result of handling a service request
pub enum AdiHandleResult {
    /// Single response with data
    Success(JsonValue),
    /// Streaming response - receiver yields chunks
    /// Each chunk is (data, done) where done=true marks final chunk
    Stream(mpsc::Receiver<(JsonValue, bool)>),
}

/// Error from service handler
#[derive(Debug, Clone)]
pub struct AdiServiceError {
    /// Error code (e.g., "not_found", "invalid_params", "internal")
    pub code: String,
    /// Human-readable error message
    pub message: String,
}

impl AdiServiceError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: "not_found".to_string(),
            message: message.into(),
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_params".to_string(),
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: "internal".to_string(),
            message: message.into(),
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: "method_not_found".to_string(),
            message: format!("Method '{}' not found", method),
        }
    }

    pub fn not_supported(message: impl Into<String>) -> Self {
        Self {
            code: "not_supported".to_string(),
            message: message.into(),
        }
    }

    pub fn subscription_failed(message: impl Into<String>) -> Self {
        Self {
            code: "subscription_failed".to_string(),
            message: message.into(),
        }
    }
}

/// Caller identity resolved from the signaling session
#[derive(Debug, Clone)]
pub struct AdiCallerContext {
    /// Owner user_id from signaling registration (from setup_token)
    pub user_id: Option<String>,
    /// Device ID of the cocoon
    pub device_id: Option<String>,
}

impl AdiCallerContext {
    pub fn anonymous() -> Self {
        Self { user_id: None, device_id: None }
    }

    /// Get user_id or return an error
    pub fn require_user_id(&self) -> Result<&str, AdiServiceError> {
        self.user_id.as_deref().ok_or_else(|| {
            AdiServiceError::new("unauthorized", "No authenticated user. Cocoon must be claimed via setup_token.")
        })
    }
}

/// Trait that plugins implement to handle requests
///
/// Plugins are registered with the router and receive method calls.
/// Each plugin has a unique ID matching its registry ID (e.g., "adi.tasks", "adi.credentials").
///
/// ## MCP-Style Features
///
/// Plugins can optionally support:
/// - **Subscriptions**: Real-time event streams via `subscribe()`
/// - **Notifications**: Async events broadcast to all connected clients
/// - **Streaming**: Long-running operations with chunked responses
#[async_trait]
pub trait AdiService: Send + Sync {
    // ========== Identity ==========

    /// Plugin identifier matching registry ID (e.g., "adi.tasks", "adi.credentials")
    fn plugin_id(&self) -> &str;

    /// Human-readable plugin name (e.g., "Task Management")
    fn name(&self) -> &str;

    /// Plugin version (semver)
    fn version(&self) -> &str;

    /// Human-readable description of the plugin
    fn description(&self) -> Option<&str> {
        None
    }

    // ========== Discovery ==========

    /// List available methods with their descriptions and schemas
    fn methods(&self) -> Vec<AdiMethodInfo>;

    /// Plugin-level capabilities
    fn capabilities(&self) -> AdiPluginCapabilities {
        AdiPluginCapabilities::default()
    }

    // ========== Request Handling ==========

    /// Handle a request
    ///
    /// # Arguments
    /// * `ctx` - Caller context (user identity from signaling)
    /// * `method` - Method name to invoke
    /// * `params` - JSON parameters for the method
    ///
    /// # Returns
    /// * `Ok(Success(data))` - Single response
    /// * `Ok(Stream(rx))` - Streaming response
    /// * `Err(error)` - Error response
    async fn handle(
        &self,
        ctx: &AdiCallerContext,
        method: &str,
        params: JsonValue,
    ) -> Result<AdiHandleResult, AdiServiceError>;

    // ========== Subscriptions (Optional) ==========

    /// List available subscription events
    ///
    /// Returns a list of event names that clients can subscribe to.
    /// Default implementation returns empty list (no subscriptions).
    fn subscription_events(&self) -> Vec<SubscriptionEventInfo> {
        vec![]
    }

    /// Subscribe to an event stream
    ///
    /// # Arguments
    /// * `event` - Event name to subscribe to
    /// * `filter` - Optional filter for events
    ///
    /// # Returns
    /// * `Ok(receiver)` - Channel that receives events
    /// * `Err(error)` - Subscription failed
    ///
    /// Default implementation returns "not supported" error.
    async fn subscribe(
        &self,
        _event: &str,
        _filter: Option<JsonValue>,
    ) -> Result<broadcast::Receiver<SubscriptionEvent>, AdiServiceError> {
        Err(AdiServiceError::not_supported("subscriptions not supported"))
    }

    // ========== Lifecycle (Optional) ==========

    /// Called when a client connects
    fn on_client_connected(&self, _client_id: &str) {}

    /// Called when a client disconnects
    fn on_client_disconnected(&self, _client_id: &str) {}
}

/// Information about a subscription event
#[derive(Debug, Clone)]
pub struct SubscriptionEventInfo {
    /// Event name (e.g., "task_created", "task_updated")
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema for event data
    pub data_schema: Option<JsonValue>,
}

/// Event sent to subscribers
#[derive(Debug, Clone)]
pub struct SubscriptionEvent {
    /// Event name
    pub event: String,
    /// Event data
    pub data: JsonValue,
}

/// Active subscription tracking
#[derive(Debug)]
pub struct ActiveSubscription {
    /// Plugin ID
    pub plugin: String,
    /// Event name
    pub event: String,
}

/// Router that dispatches ADI requests to registered plugins
pub struct AdiRouter {
    plugins: HashMap<String, Arc<dyn AdiService>>,
    /// Active subscriptions: subscription_id -> subscription info
    subscriptions: Arc<RwLock<HashMap<Uuid, ActiveSubscription>>>,
    /// Notification broadcast channel
    notification_tx: broadcast::Sender<AdiNotification>,
}

impl Default for AdiRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl AdiRouter {
    /// Create a new empty router
    pub fn new() -> Self {
        let (notification_tx, _) = broadcast::channel(256);
        Self {
            plugins: HashMap::new(),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            notification_tx,
        }
    }

    /// Get a receiver for notifications
    pub fn notification_receiver(&self) -> broadcast::Receiver<AdiNotification> {
        self.notification_tx.subscribe()
    }

    /// Broadcast a notification to all listeners
    pub fn broadcast_notification(&self, notification: AdiNotification) {
        let _ = self.notification_tx.send(notification);
    }

    /// Register a plugin handler
    ///
    /// If a plugin with the same ID already exists, it will be replaced.
    pub fn register(&mut self, plugin: Arc<dyn AdiService>) {
        let id = plugin.plugin_id().to_string();
        let caps = plugin.capabilities();
        tracing::info!(
            "Registered ADI plugin: {} v{} ({}) [streaming={}, notifications={}, subscriptions={}]",
            id,
            plugin.version(),
            plugin.name(),
            caps.streaming,
            caps.notifications,
            caps.subscriptions
        );

        let was_new = !self.plugins.contains_key(&id);
        self.plugins.insert(id.clone(), plugin);

        if was_new {
            self.broadcast_notification(AdiNotification::PluginsChanged {
                added: vec![id],
                removed: vec![],
                updated: vec![],
            });
        } else {
            self.broadcast_notification(AdiNotification::PluginsChanged {
                added: vec![],
                removed: vec![],
                updated: vec![id],
            });
        }
    }

    /// Unregister a plugin by ID
    pub fn unregister(&mut self, plugin_id: &str) -> bool {
        if self.plugins.remove(plugin_id).is_some() {
            tracing::info!("Unregistered ADI plugin: {}", plugin_id);

            self.broadcast_notification(AdiNotification::PluginsChanged {
                added: vec![],
                removed: vec![plugin_id.to_string()],
                updated: vec![],
            });
            true
        } else {
            false
        }
    }

    /// Check if a plugin is registered
    pub fn has_plugin(&self, plugin_id: &str) -> bool {
        self.plugins.contains_key(plugin_id)
    }

    /// Get a plugin by ID
    pub fn get_plugin(&self, plugin_id: &str) -> Option<Arc<dyn AdiService>> {
        self.plugins.get(plugin_id).cloned()
    }

    /// Get list of registered plugins with full metadata
    pub fn list_plugins(&self) -> Vec<AdiPluginInfo> {
        self.plugins
            .values()
            .map(|s| AdiPluginInfo {
                id: s.plugin_id().to_string(),
                name: s.name().to_string(),
                version: s.version().to_string(),
                description: s.description().map(String::from),
                methods: s.methods(),
                capabilities: s.capabilities(),
            })
            .collect()
    }

    /// Handle a discovery request
    pub fn handle_discovery(&self, discovery: AdiDiscovery) -> AdiDiscovery {
        match discovery {
            AdiDiscovery::ListPlugins { request_id } => AdiDiscovery::PluginsList {
                request_id,
                plugins: self.list_plugins(),
            },
            other => other,
        }
    }

    /// Handle a subscription request
    pub async fn handle_subscription(&self, subscription: AdiSubscription) -> AdiSubscription {
        match subscription {
            AdiSubscription::Subscribe {
                request_id,
                plugin,
                event,
                filter,
            } => {
                let svc = match self.plugins.get(&plugin) {
                    Some(s) => s,
                    None => {
                        return AdiSubscription::Error {
                            request_id,
                            code: "plugin_not_found".to_string(),
                            message: format!("Plugin '{}' not found", plugin),
                        };
                    }
                };

                if !svc.capabilities().subscriptions {
                    return AdiSubscription::Error {
                        request_id,
                        code: "not_supported".to_string(),
                        message: format!("Plugin '{}' does not support subscriptions", plugin),
                    };
                }

                match svc.subscribe(&event, filter).await {
                    Ok(_receiver) => {
                        let subscription_id = Uuid::new_v4();

                        let mut subs = self.subscriptions.write().await;
                        subs.insert(subscription_id, ActiveSubscription {
                            plugin: plugin.clone(),
                            event: event.clone(),
                        });

                        AdiSubscription::Subscribed {
                            request_id,
                            subscription_id,
                            plugin,
                            event,
                        }
                    }
                    Err(e) => AdiSubscription::Error {
                        request_id,
                        code: e.code,
                        message: e.message,
                    },
                }
            }

            AdiSubscription::Unsubscribe { subscription_id } => {
                let mut subs = self.subscriptions.write().await;
                if subs.remove(&subscription_id).is_some() {
                    AdiSubscription::Unsubscribed { subscription_id }
                } else {
                    AdiSubscription::Unsubscribed { subscription_id }
                }
            }

            other => other,
        }
    }

    /// Handle an incoming ADI request
    ///
    /// Routes the request to the appropriate plugin and returns the response.
    pub async fn handle(&self, ctx: &AdiCallerContext, request: AdiRequest) -> AdiRouterResult {
        let plugin_svc = match self.plugins.get(&request.plugin) {
            Some(s) => s,
            None => {
                return AdiRouterResult::Single(AdiResponse::PluginNotFound {
                    request_id: request.request_id,
                    plugin: request.plugin,
                });
            }
        };

        let methods = plugin_svc.methods();
        let method_exists = methods.iter().any(|m| m.name == request.method);
        if !method_exists {
            return AdiRouterResult::Single(AdiResponse::MethodNotFound {
                request_id: request.request_id,
                plugin: request.plugin,
                method: request.method,
                available_methods: methods.iter().map(|m| m.name.clone()).collect(),
            });
        }

        match plugin_svc.handle(ctx, &request.method, request.params).await {
            Ok(AdiHandleResult::Success(data)) => {
                AdiRouterResult::Single(AdiResponse::Success {
                    request_id: request.request_id,
                    plugin: request.plugin,
                    method: request.method,
                    data,
                })
            }
            Ok(AdiHandleResult::Stream(rx)) => AdiRouterResult::Stream {
                request_id: request.request_id,
                plugin: request.plugin,
                method: request.method,
                receiver: rx,
            },
            Err(e) => AdiRouterResult::Single(AdiResponse::Error {
                request_id: request.request_id,
                plugin: request.plugin,
                method: request.method,
                code: e.code,
                message: e.message,
            }),
        }
    }

    /// Notify that a client connected
    pub fn client_connected(&self, client_id: &str) {
        for plugin in self.plugins.values() {
            plugin.on_client_connected(client_id);
        }
    }

    /// Notify that a client disconnected
    pub fn client_disconnected(&self, client_id: &str) {
        for plugin in self.plugins.values() {
            plugin.on_client_disconnected(client_id);
        }
    }

    /// Get count of active subscriptions (for debugging/monitoring)
    pub async fn subscription_count(&self) -> usize {
        self.subscriptions.read().await.len()
    }

    /// Get all active subscriptions (for debugging/monitoring)
    pub async fn list_subscriptions(&self) -> Vec<(Uuid, String, String)> {
        self.subscriptions
            .read()
            .await
            .iter()
            .map(|(id, sub)| (*id, sub.plugin.clone(), sub.event.clone()))
            .collect()
    }
}

/// Result from router handling
pub enum AdiRouterResult {
    /// Single response (success, error, or not found)
    Single(AdiResponse),
    /// Streaming response
    Stream {
        request_id: Uuid,
        plugin: String,
        method: String,
        receiver: mpsc::Receiver<(JsonValue, bool)>,
    },
}

impl AdiRouterResult {
    /// Convert a single result to AdiResponse
    /// For streaming, this returns None (use the stream instead)
    pub fn into_single(self) -> Option<AdiResponse> {
        match self {
            AdiRouterResult::Single(resp) => Some(resp),
            AdiRouterResult::Stream { .. } => None,
        }
    }

    /// Check if this is a streaming result
    pub fn is_stream(&self) -> bool {
        matches!(self, AdiRouterResult::Stream { .. })
    }
}

/// Helper to create a streaming sender
///
/// Returns (sender, receiver) where sender is used by the service
/// and receiver is returned in AdiHandleResult::Stream
pub fn create_stream_channel(buffer_size: usize) -> (StreamSender, mpsc::Receiver<(JsonValue, bool)>) {
    let (tx, rx) = mpsc::channel(buffer_size);
    (StreamSender { tx }, rx)
}

/// Sender for streaming responses
pub struct StreamSender {
    tx: mpsc::Sender<(JsonValue, bool)>,
}

impl StreamSender {
    /// Send a chunk (not final)
    pub async fn send(&self, data: JsonValue) -> Result<(), ()> {
        self.tx.send((data, false)).await.map_err(|_| ())
    }

    /// Send the final chunk
    pub async fn send_final(&self, data: JsonValue) -> Result<(), ()> {
        self.tx.send((data, true)).await.map_err(|_| ())
    }

    /// Close the stream without sending a final value
    /// (receiver will see the channel close)
    pub fn close(self) {
        drop(self.tx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TestService;

    #[async_trait]
    impl AdiService for TestService {
        fn plugin_id(&self) -> &str {
            "adi.test"
        }

        fn name(&self) -> &str {
            "Test Service"
        }

        fn version(&self) -> &str {
            "1.0.0"
        }

        fn methods(&self) -> Vec<AdiMethodInfo> {
            vec![
                AdiMethodInfo {
                    name: "echo".to_string(),
                    description: "Echo back the input".to_string(),
                    streaming: false,
                    params_schema: None,
                    ..Default::default()
                },
                AdiMethodInfo {
                    name: "count".to_string(),
                    description: "Count to N (streaming)".to_string(),
                    streaming: true,
                    params_schema: None,
                    ..Default::default()
                },
            ]
        }

        async fn handle(
            &self,
            _ctx: &AdiCallerContext,
            method: &str,
            params: JsonValue,
        ) -> Result<AdiHandleResult, AdiServiceError> {
            match method {
                "echo" => Ok(AdiHandleResult::Success(params)),
                "count" => {
                    let n = params.get("n").and_then(|v| v.as_u64()).unwrap_or(5);
                    let (sender, receiver) = create_stream_channel(16);

                    tokio::spawn(async move {
                        for i in 1..=n {
                            let is_final = i == n;
                            let data = json!({ "count": i });
                            if is_final {
                                let _ = sender.send_final(data).await;
                            } else {
                                let _ = sender.send(data).await;
                            }
                        }
                    });

                    Ok(AdiHandleResult::Stream(receiver))
                }
                _ => Err(AdiServiceError::method_not_found(method)),
            }
        }
    }

    #[tokio::test]
    async fn test_router_register_and_list() {
        let mut router = AdiRouter::new();
        router.register(Arc::new(TestService));

        let plugins = router.list_plugins();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].id, "adi.test");
        assert_eq!(plugins[0].methods.len(), 2);
    }

    #[tokio::test]
    async fn test_router_handle_success() {
        let mut router = AdiRouter::new();
        router.register(Arc::new(TestService));

        let request = AdiRequest {
            request_id: Uuid::nil(),
            plugin: "adi.test".to_string(),
            method: "echo".to_string(),
            params: json!({"hello": "world"}),
        };

        let result = router.handle(&AdiCallerContext::anonymous(), request).await;
        match result {
            AdiRouterResult::Single(AdiResponse::Success { data, .. }) => {
                assert_eq!(data["hello"], "world");
            }
            _ => panic!("Expected success response"),
        }
    }

    #[tokio::test]
    async fn test_router_plugin_not_found() {
        let router = AdiRouter::new();

        let request = AdiRequest {
            request_id: Uuid::nil(),
            plugin: "nonexistent".to_string(),
            method: "test".to_string(),
            params: json!({}),
        };

        let result = router.handle(&AdiCallerContext::anonymous(), request).await;
        match result {
            AdiRouterResult::Single(AdiResponse::PluginNotFound { plugin, .. }) => {
                assert_eq!(plugin, "nonexistent");
            }
            _ => panic!("Expected plugin not found"),
        }
    }

    #[tokio::test]
    async fn test_router_method_not_found() {
        let mut router = AdiRouter::new();
        router.register(Arc::new(TestService));

        let request = AdiRequest {
            request_id: Uuid::nil(),
            plugin: "adi.test".to_string(),
            method: "nonexistent".to_string(),
            params: json!({}),
        };

        let result = router.handle(&AdiCallerContext::anonymous(), request).await;
        match result {
            AdiRouterResult::Single(AdiResponse::MethodNotFound {
                method,
                available_methods,
                ..
            }) => {
                assert_eq!(method, "nonexistent");
                assert!(available_methods.contains(&"echo".to_string()));
            }
            _ => panic!("Expected method not found"),
        }
    }

    #[tokio::test]
    async fn test_router_streaming() {
        let mut router = AdiRouter::new();
        router.register(Arc::new(TestService));

        let request = AdiRequest {
            request_id: Uuid::nil(),
            plugin: "adi.test".to_string(),
            method: "count".to_string(),
            params: json!({"n": 3}),
        };

        let result = router.handle(&AdiCallerContext::anonymous(), request).await;
        match result {
            AdiRouterResult::Stream { mut receiver, .. } => {
                let mut chunks = Vec::new();
                while let Some((data, done)) = receiver.recv().await {
                    chunks.push((data, done));
                    if done {
                        break;
                    }
                }
                assert_eq!(chunks.len(), 3);
                assert_eq!(chunks[0].0["count"], 1);
                assert!(!chunks[0].1); // not done
                assert_eq!(chunks[2].0["count"], 3);
                assert!(chunks[2].1); // done
            }
            _ => panic!("Expected streaming response"),
        }
    }
}
