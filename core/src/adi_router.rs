//! ADI Service Router
//!
//! Generic router for ADI services. Services register themselves with the router
//! and receive requests via the "adi" WebRTC data channel.
//!
//! The router is format-agnostic: it reads a binary frame header (JSON with plugin/method/request_id)
//! for routing, then passes raw bytes through to the target plugin untouched.
//! Each plugin decides its own payload serialization format.

use async_trait::async_trait;
use bytes::Bytes;
use crate::adi_frame::{self, RequestHeader, ResponseStatus};
use serde::{Serialize, Deserialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
use uuid::Uuid;

// Re-export all shared types from lib-adi-service
pub use lib_adi_service::{
    AdiCallerContext, AdiHandleResult, AdiService, AdiServiceError,
    AdiMethodInfo, AdiPluginCapabilities, AdiPluginInfo,
    StreamSender, SubscriptionEvent, SubscriptionEventInfo,
    create_stream_channel,
};

// ── Legacy JSON types (kept for discovery/subscriptions which remain text-based) ──

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

#[derive(Debug)]
pub struct ActiveSubscription {
    pub plugin: String,
    pub event: String,
}

pub struct AdiRouter {
    plugins: HashMap<String, Arc<dyn AdiService>>,
    subscriptions: Arc<RwLock<HashMap<Uuid, ActiveSubscription>>>,
    notification_tx: broadcast::Sender<AdiNotification>,
}

impl Default for AdiRouter {
    fn default() -> Self { Self::new() }
}

impl AdiRouter {
    pub fn new() -> Self {
        let (notification_tx, _) = broadcast::channel(256);
        Self {
            plugins: HashMap::new(),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            notification_tx,
        }
    }

    pub fn notification_receiver(&self) -> broadcast::Receiver<AdiNotification> {
        self.notification_tx.subscribe()
    }

    pub fn broadcast_notification(&self, notification: AdiNotification) {
        let _ = self.notification_tx.send(notification);
    }

    pub fn register(&mut self, plugin: Arc<dyn AdiService>) {
        let id = plugin.plugin_id().to_string();
        let caps = plugin.capabilities();
        tracing::info!(
            "Registered ADI plugin: {} v{} ({}) [streaming={}, notifications={}, subscriptions={}]",
            id, plugin.version(), plugin.name(),
            caps.streaming, caps.notifications, caps.subscriptions
        );

        let was_new = !self.plugins.contains_key(&id);
        self.plugins.insert(id.clone(), plugin);

        if was_new {
            self.broadcast_notification(AdiNotification::PluginsChanged {
                added: vec![id], removed: vec![], updated: vec![],
            });
        } else {
            self.broadcast_notification(AdiNotification::PluginsChanged {
                added: vec![], removed: vec![], updated: vec![id],
            });
        }
    }

    pub fn unregister(&mut self, plugin_id: &str) -> bool {
        if self.plugins.remove(plugin_id).is_some() {
            tracing::info!("Unregistered ADI plugin: {}", plugin_id);
            self.broadcast_notification(AdiNotification::PluginsChanged {
                added: vec![], removed: vec![plugin_id.to_string()], updated: vec![],
            });
            true
        } else {
            false
        }
    }

    pub fn has_plugin(&self, plugin_id: &str) -> bool {
        self.plugins.contains_key(plugin_id)
    }

    pub fn get_plugin(&self, plugin_id: &str) -> Option<Arc<dyn AdiService>> {
        self.plugins.get(plugin_id).cloned()
    }

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

    pub fn handle_discovery(&self, discovery: AdiDiscovery) -> AdiDiscovery {
        match discovery {
            AdiDiscovery::ListPlugins { request_id } => AdiDiscovery::PluginsList {
                request_id,
                plugins: self.list_plugins(),
            },
            other => other,
        }
    }

    pub async fn handle_subscription(&self, subscription: AdiSubscription) -> AdiSubscription {
        match subscription {
            AdiSubscription::Subscribe { request_id, plugin, event, filter } => {
                let svc = match self.plugins.get(&plugin) {
                    Some(s) => s,
                    None => return AdiSubscription::Error {
                        request_id,
                        code: "plugin_not_found".to_string(),
                        message: format!("Plugin '{}' not found", plugin),
                    },
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

                        AdiSubscription::Subscribed { request_id, subscription_id, plugin, event }
                    }
                    Err(e) => AdiSubscription::Error {
                        request_id, code: e.code, message: e.message,
                    },
                }
            }

            AdiSubscription::Unsubscribe { subscription_id } => {
                let mut subs = self.subscriptions.write().await;
                subs.remove(&subscription_id);
                AdiSubscription::Unsubscribed { subscription_id }
            }

            other => other,
        }
    }

    /// Handle a binary-framed ADI request.
    ///
    /// Parses the frame header, routes to the plugin, and returns a complete
    /// binary response frame ready to send over the wire.
    pub async fn handle_binary(&self, ctx: &AdiCallerContext, raw: &[u8]) -> AdiRouterBinaryResult {
        let (header, payload) = match adi_frame::parse_request(raw) {
            Ok(r) => r,
            Err(e) => {
                return AdiRouterBinaryResult::Single(
                    adi_frame::router_error(Uuid::nil(), ResponseStatus::InvalidRequest, &e.to_string()),
                );
            }
        };

        let plugin_svc = match self.plugins.get(&header.plugin) {
            Some(s) => s,
            None => {
                return AdiRouterBinaryResult::Single(adi_frame::router_error(
                    header.id,
                    ResponseStatus::PluginNotFound,
                    &format!("Plugin '{}' not found", header.plugin),
                ));
            }
        };

        let methods = plugin_svc.methods();
        if !methods.iter().any(|m| m.name == header.method) {
            let available: Vec<&str> = methods.iter().map(|m| m.name.as_str()).collect();
            return AdiRouterBinaryResult::Single(adi_frame::router_error(
                header.id,
                ResponseStatus::MethodNotFound,
                &format!("Method '{}' not found. Available: {:?}", header.method, available),
            ));
        }

        match plugin_svc.handle(ctx, &header.method, payload).await {
            Ok(AdiHandleResult::Success(data)) => {
                AdiRouterBinaryResult::Single(adi_frame::success_response(header.id, &data))
            }
            Ok(AdiHandleResult::Stream(rx)) => {
                AdiRouterBinaryResult::Stream { request_id: header.id, receiver: rx }
            }
            Err(e) => {
                AdiRouterBinaryResult::Single(adi_frame::error_response(header.id, &e.to_payload()))
            }
        }
    }

    pub fn client_connected(&self, client_id: &str) {
        for plugin in self.plugins.values() {
            plugin.on_client_connected(client_id);
        }
    }

    pub fn client_disconnected(&self, client_id: &str) {
        for plugin in self.plugins.values() {
            plugin.on_client_disconnected(client_id);
        }
    }

    pub async fn subscription_count(&self) -> usize {
        self.subscriptions.read().await.len()
    }

    pub async fn list_subscriptions(&self) -> Vec<(Uuid, String, String)> {
        self.subscriptions
            .read()
            .await
            .iter()
            .map(|(id, sub)| (*id, sub.plugin.clone(), sub.event.clone()))
            .collect()
    }
}

/// Result from binary-framed router handling.
pub enum AdiRouterBinaryResult {
    /// Single response frame (ready to send)
    Single(Bytes),
    /// Streaming response
    Stream {
        request_id: Uuid,
        receiver: mpsc::Receiver<(Bytes, bool)>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TestService;

    #[async_trait]
    impl AdiService for TestService {
        fn plugin_id(&self) -> &str { "adi.test" }
        fn name(&self) -> &str { "Test Service" }
        fn version(&self) -> &str { "1.0.0" }

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
            payload: Bytes,
        ) -> Result<AdiHandleResult, AdiServiceError> {
            match method {
                "echo" => Ok(AdiHandleResult::Success(payload)),
                "count" => {
                    let params: JsonValue = serde_json::from_slice(&payload)
                        .map_err(|e| AdiServiceError::invalid_params(e.to_string()))?;
                    let n = params.get("n").and_then(|v| v.as_u64()).unwrap_or(5);
                    let (sender, receiver) = create_stream_channel(16);

                    tokio::spawn(async move {
                        for i in 1..=n {
                            let is_final = i == n;
                            let data = Bytes::from(serde_json::to_vec(&json!({ "count": i })).unwrap());
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

    fn build_frame(plugin: &str, method: &str, payload: &[u8]) -> Vec<u8> {
        let header = RequestHeader {
            v: 1,
            id: Uuid::nil(),
            plugin: plugin.to_string(),
            method: method.to_string(),
            stream: false,
        };
        let header_json = serde_json::to_vec(&header).unwrap();
        let mut buf = Vec::with_capacity(4 + header_json.len() + payload.len());
        buf.extend_from_slice(&(header_json.len() as u32).to_be_bytes());
        buf.extend_from_slice(&header_json);
        buf.extend_from_slice(payload);
        buf
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

        let payload = serde_json::to_vec(&json!({"hello": "world"})).unwrap();
        let frame = build_frame("adi.test", "echo", &payload);

        let result = router.handle_binary(&AdiCallerContext::anonymous(), &frame).await;
        match result {
            AdiRouterBinaryResult::Single(response_frame) => {
                let header_len = u32::from_be_bytes([
                    response_frame[0], response_frame[1], response_frame[2], response_frame[3],
                ]) as usize;
                let header: adi_frame::ResponseHeader =
                    serde_json::from_slice(&response_frame[4..4 + header_len]).unwrap();
                let resp_payload = &response_frame[4 + header_len..];

                assert_eq!(header.status, ResponseStatus::Success);
                let data: JsonValue = serde_json::from_slice(resp_payload).unwrap();
                assert_eq!(data["hello"], "world");
            }
            _ => panic!("Expected single response"),
        }
    }

    #[tokio::test]
    async fn test_router_plugin_not_found() {
        let router = AdiRouter::new();
        let frame = build_frame("nonexistent", "test", b"{}");

        let result = router.handle_binary(&AdiCallerContext::anonymous(), &frame).await;
        match result {
            AdiRouterBinaryResult::Single(response_frame) => {
                let header_len = u32::from_be_bytes([
                    response_frame[0], response_frame[1], response_frame[2], response_frame[3],
                ]) as usize;
                let header: adi_frame::ResponseHeader =
                    serde_json::from_slice(&response_frame[4..4 + header_len]).unwrap();
                assert_eq!(header.status, ResponseStatus::PluginNotFound);
            }
            _ => panic!("Expected single response"),
        }
    }

    #[tokio::test]
    async fn test_router_method_not_found() {
        let mut router = AdiRouter::new();
        router.register(Arc::new(TestService));

        let frame = build_frame("adi.test", "nonexistent", b"{}");

        let result = router.handle_binary(&AdiCallerContext::anonymous(), &frame).await;
        match result {
            AdiRouterBinaryResult::Single(response_frame) => {
                let header_len = u32::from_be_bytes([
                    response_frame[0], response_frame[1], response_frame[2], response_frame[3],
                ]) as usize;
                let header: adi_frame::ResponseHeader =
                    serde_json::from_slice(&response_frame[4..4 + header_len]).unwrap();
                assert_eq!(header.status, ResponseStatus::MethodNotFound);
            }
            _ => panic!("Expected single response"),
        }
    }

    #[tokio::test]
    async fn test_router_streaming() {
        let mut router = AdiRouter::new();
        router.register(Arc::new(TestService));

        let payload = serde_json::to_vec(&json!({"n": 3})).unwrap();
        let frame = build_frame("adi.test", "count", &payload);

        let result = router.handle_binary(&AdiCallerContext::anonymous(), &frame).await;
        match result {
            AdiRouterBinaryResult::Stream { mut receiver, .. } => {
                let mut chunks = Vec::new();
                while let Some((data, done)) = receiver.recv().await {
                    let val: JsonValue = serde_json::from_slice(&data).unwrap();
                    chunks.push((val, done));
                    if done { break; }
                }
                assert_eq!(chunks.len(), 3);
                assert_eq!(chunks[0].0["count"], 1);
                assert!(!chunks[0].1);
                assert_eq!(chunks[2].0["count"], 3);
                assert!(chunks[2].1);
            }
            _ => panic!("Expected streaming response"),
        }
    }
}
