//! WebRTC session handling for cocoon
//!
//! Provides WebRTC peer connection management for low-latency, direct
//! communication between browser clients and cocoons.
//!
//! ## Configuration
//!
//! ICE servers can be configured via environment variables:
//!
//! - `WEBRTC_ICE_SERVERS`: Comma-separated list of STUN/TURN server URLs
//!   Example: `stun:stun.l.google.com:19302,turn:turn.example.com:3478`
//!
//! - `WEBRTC_TURN_USERNAME`: Username for TURN server authentication
//!
//! - `WEBRTC_TURN_CREDENTIAL`: Credential/password for TURN server authentication
//!
//! If no ICE servers are configured, defaults to Google's public STUN server.

use crate::filesystem::{FileSystemRequest, handle_request as handle_fs_request};
use lib_signaling_protocol::SignalingMessage;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

/// Build ICE server configuration from environment variables
///
/// Environment variables:
/// - `WEBRTC_ICE_SERVERS`: Comma-separated list of STUN/TURN URLs
/// - `WEBRTC_TURN_USERNAME`: Username for TURN authentication
/// - `WEBRTC_TURN_CREDENTIAL`: Credential for TURN authentication
fn build_ice_servers() -> Vec<RTCIceServer> {
    let ice_servers_env = std::env::var("WEBRTC_ICE_SERVERS").ok();
    let turn_username = std::env::var("WEBRTC_TURN_USERNAME").ok();
    let turn_credential = std::env::var("WEBRTC_TURN_CREDENTIAL").ok();

    let urls: Vec<String> = ice_servers_env
        .as_ref()
        .map(|s| s.split(',').map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).collect())
        .unwrap_or_default();

    if urls.is_empty() {
        // Default to Google's public STUN server
        tracing::info!("No WEBRTC_ICE_SERVERS configured, using default Google STUN server");
        return vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            ..Default::default()
        }];
    }

    // Separate STUN and TURN servers
    let stun_urls: Vec<String> = urls.iter().filter(|u| u.starts_with("stun:")).cloned().collect();
    let turn_urls: Vec<String> = urls.iter().filter(|u| u.starts_with("turn:") || u.starts_with("turns:")).cloned().collect();

    let mut ice_servers = Vec::new();

    // Add STUN servers (no auth needed)
    if !stun_urls.is_empty() {
        tracing::info!("Configured {} STUN server(s): {:?}", stun_urls.len(), stun_urls);
        ice_servers.push(RTCIceServer {
            urls: stun_urls,
            ..Default::default()
        });
    }

    // Add TURN servers (with auth if provided)
    if !turn_urls.is_empty() {
        let has_credentials = turn_username.is_some() && turn_credential.is_some();
        tracing::info!(
            "Configured {} TURN server(s): {:?} (credentials: {})",
            turn_urls.len(),
            turn_urls,
            if has_credentials { "provided" } else { "none" }
        );

        ice_servers.push(RTCIceServer {
            urls: turn_urls,
            username: turn_username.unwrap_or_default(),
            credential: turn_credential.unwrap_or_default(),
            ..Default::default()
        });
    }

    // If we somehow ended up with an empty list, add default STUN
    if ice_servers.is_empty() {
        tracing::warn!("No valid ICE servers found, falling back to default Google STUN");
        ice_servers.push(RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            ..Default::default()
        });
    }

    ice_servers
}

/// WebRTC session state
pub struct WebRtcSession {
    pub session_id: String,
    pub peer_connection: Arc<RTCPeerConnection>,
    pub data_channels: HashMap<String, Arc<RTCDataChannel>>,
    pub state: String,
}

/// WebRTC session manager
pub struct WebRtcManager {
    sessions: Arc<Mutex<HashMap<String, WebRtcSession>>>,
    signaling_tx: mpsc::UnboundedSender<SignalingMessage>,
    /// Timeout for closing peer connections (default: 5 seconds)
    close_timeout: std::time::Duration,
}

impl WebRtcManager {
    /// Create a new WebRTC manager
    pub fn new(signaling_tx: mpsc::UnboundedSender<SignalingMessage>) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            signaling_tx,
            close_timeout: std::time::Duration::from_secs(5),
        }
    }

    /// Create a new WebRTC manager with custom close timeout
    #[cfg(test)]
    pub fn with_close_timeout(
        signaling_tx: mpsc::UnboundedSender<SignalingMessage>,
        close_timeout: std::time::Duration,
    ) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            signaling_tx,
            close_timeout,
        }
    }

    /// Create a new WebRTC peer connection for a session
    pub async fn create_session(&self, session_id: String) -> Result<(), String> {
        let ice_servers = build_ice_servers();
        let config = RTCConfiguration {
            ice_servers,
            ..Default::default()
        };

        // Create a MediaEngine
        let mut media_engine = MediaEngine::default();

        // Create an interceptor registry
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine)
            .map_err(|e| format!("Failed to register interceptors: {}", e))?;

        // Create a SettingEngine and enable Detach mode for data channels
        let mut setting_engine = SettingEngine::default();
        setting_engine.detach_data_channels();

        // Create the API
        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .with_setting_engine(setting_engine)
            .build();

        // Create the peer connection
        let peer_connection = api
            .new_peer_connection(config)
            .await
            .map_err(|e| format!("Failed to create peer connection: {}", e))?;

        let peer_connection = Arc::new(peer_connection);

        // Set up ICE candidate handler
        let session_id_clone = session_id.clone();
        let signaling_tx_clone = self.signaling_tx.clone();
        peer_connection.on_ice_candidate(Box::new(move |candidate| {
            let session_id = session_id_clone.clone();
            let tx = signaling_tx_clone.clone();

            Box::pin(async move {
                if let Some(c) = candidate {
                    if let Ok(json) = c.to_json() {
                        // Log ICE candidate type for debugging connectivity issues
                        let candidate_type = if json.candidate.contains("typ host") {
                            "host"
                        } else if json.candidate.contains("typ srflx") {
                            "srflx (STUN)"
                        } else if json.candidate.contains("typ relay") {
                            "relay (TURN)"
                        } else if json.candidate.contains("typ prflx") {
                            "prflx"
                        } else {
                            "unknown"
                        };
                        tracing::debug!(
                            "🧊 ICE candidate gathered for session {}: type={}, mid={:?}",
                            session_id,
                            candidate_type,
                            json.sdp_mid
                        );

                        let _ = tx.send(SignalingMessage::WebRtcIceCandidate {
                            session_id,
                            candidate: json.candidate,
                            sdp_mid: json.sdp_mid,
                            sdp_mline_index: json.sdp_mline_index.map(|i| i as u32),
                        });
                    }
                } else {
                    // End of ICE gathering
                    tracing::debug!("🧊 ICE gathering complete for session {}", session_id);
                }
            })
        }));

        // Set up ICE gathering state handler for debugging
        let session_id_clone = session_id.clone();
        peer_connection.on_ice_gathering_state_change(Box::new(move |state| {
            let session_id = session_id_clone.clone();
            Box::pin(async move {
                tracing::debug!(
                    "🧊 ICE gathering state for session {}: {:?}",
                    session_id,
                    state
                );
            })
        }));

        // Set up ICE connection state handler for debugging
        let session_id_clone = session_id.clone();
        peer_connection.on_ice_connection_state_change(Box::new(move |state| {
            let session_id = session_id_clone.clone();
            Box::pin(async move {
                tracing::info!(
                    "🧊 ICE connection state for session {}: {:?}",
                    session_id,
                    state
                );
            })
        }));

        // Set up connection state handler
        let session_id_clone = session_id.clone();
        let signaling_tx_clone = self.signaling_tx.clone();
        let sessions_clone = self.sessions.clone();
        peer_connection.on_peer_connection_state_change(Box::new(move |state| {
            let session_id = session_id_clone.clone();
            let tx = signaling_tx_clone.clone();
            let sessions = sessions_clone.clone();

            Box::pin(async move {
                tracing::info!("WebRTC session {} state changed: {:?}", session_id, state);

                match state {
                    RTCPeerConnectionState::Connected => {
                        tracing::info!("✅ WebRTC session {} connected successfully!", session_id);
                        if let Some(session) = sessions.lock().await.get_mut(&session_id) {
                            session.state = "connected".to_string();
                        }
                    }
                    RTCPeerConnectionState::Disconnected
                    | RTCPeerConnectionState::Failed
                    | RTCPeerConnectionState::Closed => {
                        let reason = match state {
                            RTCPeerConnectionState::Disconnected => "disconnected",
                            RTCPeerConnectionState::Failed => {
                                tracing::warn!(
                                    "❌ WebRTC session {} failed - this often indicates ICE connectivity issues. \
                                    Check WEBRTC_ICE_SERVERS config and ensure TURN server is available for NAT traversal.",
                                    session_id
                                );
                                "failed"
                            }
                            RTCPeerConnectionState::Closed => "closed",
                            _ => "unknown",
                        };

                        let _ = tx.send(SignalingMessage::WebRtcSessionEnded {
                            session_id: session_id.clone(),
                            reason: Some(reason.to_string()),
                        });

                        sessions.lock().await.remove(&session_id);
                    }
                    _ => {}
                }
            })
        }));

        // Set up data channel handler
        let session_id_clone = session_id.clone();
        let signaling_tx_clone = self.signaling_tx.clone();
        let sessions_clone = self.sessions.clone();
        peer_connection.on_data_channel(Box::new(move |dc| {
            let session_id = session_id_clone.clone();
            let tx = signaling_tx_clone.clone();
            let sessions = sessions_clone.clone();
            let dc_label = dc.label().to_string();

            Box::pin(async move {
                tracing::info!(
                    "WebRTC session {} data channel opened: {}",
                    session_id,
                    dc_label
                );

                // Store the data channel
                if let Some(session) = sessions.lock().await.get_mut(&session_id) {
                    session.data_channels.insert(dc_label.clone(), dc.clone());
                }

                // Set up message handler
                let dc_label_clone = dc_label.clone();
                let session_id_clone = session_id.clone();
                let tx_clone = tx.clone();
                let dc_clone = dc.clone();
                dc.on_message(Box::new(move |msg: DataChannelMessage| {
                    let session_id = session_id_clone.clone();
                    let channel = dc_label_clone.clone();
                    let tx = tx_clone.clone();
                    let dc_for_response = dc_clone.clone();

                    Box::pin(async move {
                        let (data, binary) = if msg.is_string {
                            (String::from_utf8_lossy(&msg.data).to_string(), false)
                        } else {
                            (base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &msg.data), true)
                        };

                        // Handle "file" channel for filesystem operations
                        if channel == "file" {
                            tracing::debug!("📁 File system request received: {} bytes", data.len());
                            
                            // Parse the request
                            match serde_json::from_str::<FileSystemRequest>(&data) {
                                Ok(request) => {
                                    // Handle the filesystem request
                                    let response = handle_fs_request(request).await;
                                    
                                    // Serialize response
                                    match serde_json::to_string(&response) {
                                        Ok(response_json) => {
                                            // Send response back through the data channel
                                            let response_len = response_json.len();
                                            if let Err(e) = dc_for_response.send(&response_json.into_bytes().into()).await {
                                                tracing::error!("❌ Failed to send filesystem response: {}", e);
                                            } else {
                                                tracing::debug!("📤 Filesystem response sent: {} bytes", response_len);
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!("❌ Failed to serialize filesystem response: {}", e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("⚠️ Invalid filesystem request: {}", e);
                                    // Send error response
                                    let error_response = serde_json::json!({
                                        "type": "fs_error",
                                        "request_id": "",
                                        "code": "invalid_request",
                                        "message": format!("Failed to parse request: {}", e)
                                    });
                                    if let Ok(error_json) = serde_json::to_string(&error_response) {
                                        let _ = dc_for_response.send(&error_json.into_bytes().into()).await;
                                    }
                                }
                            }
                            return;
                        }

                        // Forward other channels through signaling (for processing)
                        let _ = tx.send(SignalingMessage::WebRtcData {
                            session_id,
                            channel,
                            data,
                            binary,
                        });
                    })
                }));
            })
        }));

        // Store the session
        let session = WebRtcSession {
            session_id: session_id.clone(),
            peer_connection,
            data_channels: HashMap::new(),
            state: "pending".to_string(),
        };

        self.sessions.lock().await.insert(session_id, session);

        Ok(())
    }

    /// Handle an incoming SDP offer and create an answer
    pub async fn handle_offer(&self, session_id: &str, sdp: &str) -> Result<String, String> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        // Parse the offer
        let offer = RTCSessionDescription::offer(sdp.to_string())
            .map_err(|e| format!("Failed to parse SDP offer: {}", e))?;

        // Set remote description
        session
            .peer_connection
            .set_remote_description(offer)
            .await
            .map_err(|e| format!("Failed to set remote description: {}", e))?;

        // Create answer
        let answer = session
            .peer_connection
            .create_answer(None)
            .await
            .map_err(|e| format!("Failed to create answer: {}", e))?;

        // Set local description
        session
            .peer_connection
            .set_local_description(answer.clone())
            .await
            .map_err(|e| format!("Failed to set local description: {}", e))?;

        Ok(answer.sdp)
    }

    /// Add an ICE candidate from remote peer
    pub async fn add_ice_candidate(
        &self,
        session_id: &str,
        candidate: &str,
        sdp_mid: Option<&str>,
        sdp_mline_index: Option<u32>,
    ) -> Result<(), String> {
        // Log remote ICE candidate for debugging
        let candidate_type = if candidate.contains("typ host") {
            "host"
        } else if candidate.contains("typ srflx") {
            "srflx (STUN)"
        } else if candidate.contains("typ relay") {
            "relay (TURN)"
        } else if candidate.contains("typ prflx") {
            "prflx"
        } else {
            "unknown"
        };
        tracing::debug!(
            "🧊 Remote ICE candidate received for session {}: type={}, mid={:?}",
            session_id,
            candidate_type,
            sdp_mid
        );

        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        let ice_candidate = webrtc::ice_transport::ice_candidate::RTCIceCandidateInit {
            candidate: candidate.to_string(),
            sdp_mid: sdp_mid.map(|s| s.to_string()),
            sdp_mline_index: sdp_mline_index.map(|i| i as u16),
            ..Default::default()
        };

        session
            .peer_connection
            .add_ice_candidate(ice_candidate)
            .await
            .map_err(|e| format!("Failed to add ICE candidate: {}", e))?;

        Ok(())
    }

    /// Send data through a data channel
    pub async fn send_data(
        &self,
        session_id: &str,
        channel: &str,
        data: &str,
        binary: bool,
    ) -> Result<(), String> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        let dc = session
            .data_channels
            .get(channel)
            .ok_or_else(|| format!("Data channel {} not found", channel))?;

        let bytes = if binary {
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data)
                .map_err(|e| format!("Failed to decode base64: {}", e))?
        } else {
            data.as_bytes().to_vec()
        };

        dc.send(&bytes.into())
            .await
            .map_err(|e| format!("Failed to send data: {}", e))?;

        Ok(())
    }

    /// Close a session
    ///
    /// Uses a timeout for the peer connection close to prevent hanging
    /// when the connection was never fully established.
    pub async fn close_session(&self, session_id: &str) -> Result<(), String> {
        if let Some(session) = self.sessions.lock().await.remove(session_id) {
            // Use a timeout for close() as it can hang if the connection
            // was never fully established (common in tests or rapid page refreshes)
            let close_result = tokio::time::timeout(
                self.close_timeout,
                session.peer_connection.close(),
            )
            .await;

            match close_result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(
                        "Failed to close peer connection for session {}: {}",
                        session_id,
                        e
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        "Timeout closing peer connection for session {} (this is often normal)",
                        session_id
                    );
                    // Don't return error - the session is already removed from the map
                }
            }
        }
        Ok(())
    }

    /// Get the list of active sessions
    pub async fn list_sessions(&self) -> Vec<String> {
        self.sessions
            .lock()
            .await
            .keys()
            .cloned()
            .collect()
    }

    /// Get the number of active sessions
    pub async fn session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    /// Check if a session exists
    pub async fn session_exists(&self, session_id: &str) -> bool {
        self.sessions.lock().await.contains_key(session_id)
    }

    /// Get session state
    pub async fn get_session_state(&self, session_id: &str) -> Option<String> {
        self.sessions
            .lock()
            .await
            .get(session_id)
            .map(|s| s.state.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// Helper to create a WebRtcManager for testing
    /// Uses a short close timeout (100ms) to speed up tests
    fn create_test_manager() -> (WebRtcManager, mpsc::UnboundedReceiver<SignalingMessage>) {
        let (tx, rx) = mpsc::unbounded_channel();
        // Use very short timeout for tests - close() will timeout but that's fine
        // since we're just testing the session management logic
        let manager =
            WebRtcManager::with_close_timeout(tx, std::time::Duration::from_millis(100));
        (manager, rx)
    }

    #[tokio::test]
    async fn test_create_single_session() {
        let (manager, _rx) = create_test_manager();

        let result = manager.create_session("session-1".to_string()).await;
        assert!(result.is_ok(), "Failed to create session: {:?}", result);

        assert!(manager.session_exists("session-1").await);
        assert_eq!(manager.session_count().await, 1);
    }

    #[tokio::test]
    async fn test_create_multiple_sessions_sequentially() {
        let (manager, _rx) = create_test_manager();

        // Create 5 sessions sequentially
        for i in 1..=5 {
            let session_id = format!("session-{}", i);
            let result = manager.create_session(session_id.clone()).await;
            assert!(
                result.is_ok(),
                "Failed to create session {}: {:?}",
                i,
                result
            );
            assert!(manager.session_exists(&session_id).await);
        }

        assert_eq!(manager.session_count().await, 5);

        // Verify all sessions exist
        let sessions = manager.list_sessions().await;
        for i in 1..=5 {
            assert!(
                sessions.contains(&format!("session-{}", i)),
                "Session {} not found in list",
                i
            );
        }
    }

    #[tokio::test]
    async fn test_create_multiple_sessions_concurrently() {
        let (manager, _rx) = create_test_manager();
        let manager = Arc::new(manager);

        // Create 10 sessions concurrently
        let mut handles = vec![];
        for i in 1..=10 {
            let manager_clone = manager.clone();
            let handle = tokio::spawn(async move {
                let session_id = format!("concurrent-session-{}", i);
                manager_clone.create_session(session_id).await
            });
            handles.push(handle);
        }

        // Wait for all to complete
        let results: Vec<_> = futures::future::join_all(handles).await;

        // All should succeed
        for (i, result) in results.into_iter().enumerate() {
            let inner_result = result.expect("Task panicked");
            assert!(
                inner_result.is_ok(),
                "Concurrent session {} failed: {:?}",
                i + 1,
                inner_result
            );
        }

        assert_eq!(manager.session_count().await, 10);
    }

    #[tokio::test]
    async fn test_close_session_and_cleanup() {
        let (manager, _rx) = create_test_manager();

        // Create a session
        manager
            .create_session("session-to-close".to_string())
            .await
            .expect("Failed to create session");
        assert!(manager.session_exists("session-to-close").await);

        // Close it
        manager
            .close_session("session-to-close")
            .await
            .expect("Failed to close session");

        // Verify it's removed
        assert!(!manager.session_exists("session-to-close").await);
        assert_eq!(manager.session_count().await, 0);
    }

    #[tokio::test]
    async fn test_recreate_session_after_close() {
        let (manager, _rx) = create_test_manager();

        // Create initial session
        manager
            .create_session("recyclable-session".to_string())
            .await
            .expect("Failed to create initial session");
        assert!(manager.session_exists("recyclable-session").await);

        // Close it
        manager
            .close_session("recyclable-session")
            .await
            .expect("Failed to close session");
        assert!(!manager.session_exists("recyclable-session").await);

        // Recreate with same ID - THIS IS THE KEY TEST FOR THE BUG
        let result = manager
            .create_session("recyclable-session".to_string())
            .await;
        assert!(
            result.is_ok(),
            "Failed to recreate session after close: {:?}",
            result
        );
        assert!(manager.session_exists("recyclable-session").await);
    }

    #[tokio::test]
    async fn test_session_lifecycle_multiple_cycles() {
        let (manager, _rx) = create_test_manager();

        // Run 5 create-close cycles on the same session ID
        for cycle in 1..=5 {
            let result = manager
                .create_session("lifecycle-test".to_string())
                .await;
            assert!(
                result.is_ok(),
                "Cycle {}: Failed to create session: {:?}",
                cycle,
                result
            );
            assert!(
                manager.session_exists("lifecycle-test").await,
                "Cycle {}: Session should exist after creation",
                cycle
            );

            manager
                .close_session("lifecycle-test")
                .await
                .expect(&format!("Cycle {}: Failed to close session", cycle));
            assert!(
                !manager.session_exists("lifecycle-test").await,
                "Cycle {}: Session should not exist after close",
                cycle
            );
        }
    }

    #[tokio::test]
    async fn test_close_nonexistent_session() {
        let (manager, _rx) = create_test_manager();

        // Should not error on closing non-existent session
        let result = manager.close_session("nonexistent").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_multiple_sessions_independent_lifecycle() {
        let (manager, _rx) = create_test_manager();

        // Create 3 sessions
        manager
            .create_session("session-a".to_string())
            .await
            .expect("Failed to create session-a");
        manager
            .create_session("session-b".to_string())
            .await
            .expect("Failed to create session-b");
        manager
            .create_session("session-c".to_string())
            .await
            .expect("Failed to create session-c");

        assert_eq!(manager.session_count().await, 3);

        // Close middle session
        manager
            .close_session("session-b")
            .await
            .expect("Failed to close session-b");

        // Verify others still exist
        assert!(manager.session_exists("session-a").await);
        assert!(!manager.session_exists("session-b").await);
        assert!(manager.session_exists("session-c").await);
        assert_eq!(manager.session_count().await, 2);

        // Recreate session-b
        manager
            .create_session("session-b".to_string())
            .await
            .expect("Failed to recreate session-b");

        assert_eq!(manager.session_count().await, 3);
        assert!(manager.session_exists("session-b").await);
    }

    #[tokio::test]
    async fn test_initial_session_state() {
        let (manager, _rx) = create_test_manager();

        manager
            .create_session("state-test".to_string())
            .await
            .expect("Failed to create session");

        let state = manager.get_session_state("state-test").await;
        assert_eq!(state, Some("pending".to_string()));
    }

    #[tokio::test]
    async fn test_session_not_found_returns_none() {
        let (manager, _rx) = create_test_manager();

        let state = manager.get_session_state("nonexistent").await;
        assert!(state.is_none());
    }

    #[tokio::test]
    async fn test_rapid_create_close_cycles() {
        let (manager, _rx) = create_test_manager();
        let manager = Arc::new(manager);

        // Simulate rapid page refresh scenario - 20 rapid cycles
        for i in 1..=20 {
            let result = manager.create_session(format!("rapid-{}", i)).await;
            assert!(
                result.is_ok(),
                "Rapid cycle {}: create failed: {:?}",
                i,
                result
            );

            // Immediately close
            manager
                .close_session(&format!("rapid-{}", i))
                .await
                .expect(&format!("Rapid cycle {}: close failed", i));
        }

        // All should be cleaned up
        assert_eq!(manager.session_count().await, 0);
    }

    #[tokio::test]
    async fn test_concurrent_create_and_close() {
        let (manager, _rx) = create_test_manager();
        let manager = Arc::new(manager);

        // Create 10 sessions
        for i in 1..=10 {
            manager
                .create_session(format!("cc-session-{}", i))
                .await
                .expect("Failed to create session");
        }

        // Concurrently close half and create new ones
        let mut handles = vec![];

        // Close odd sessions
        for i in (1..=10).step_by(2) {
            let manager_clone = manager.clone();
            let handle = tokio::spawn(async move {
                manager_clone
                    .close_session(&format!("cc-session-{}", i))
                    .await
            });
            handles.push(handle);
        }

        // Create new sessions
        for i in 11..=15 {
            let manager_clone = manager.clone();
            let handle = tokio::spawn(async move {
                manager_clone
                    .create_session(format!("cc-session-{}", i))
                    .await
            });
            handles.push(handle);
        }

        // Wait for all
        let results: Vec<_> = futures::future::join_all(handles).await;

        // All operations should succeed
        for result in results {
            let inner = result.expect("Task panicked");
            assert!(inner.is_ok(), "Operation failed: {:?}", inner);
        }

        // Should have: 5 even (2,4,6,8,10) + 5 new (11-15) = 10 sessions
        assert_eq!(manager.session_count().await, 10);
    }

    #[tokio::test]
    async fn test_duplicate_session_id_overwrites() {
        let (manager, _rx) = create_test_manager();

        // Create session
        manager
            .create_session("duplicate-test".to_string())
            .await
            .expect("Failed to create first session");

        // Create again with same ID (should overwrite)
        let result = manager.create_session("duplicate-test".to_string()).await;
        assert!(result.is_ok(), "Second create should succeed");

        // Should still be just 1 session
        assert_eq!(manager.session_count().await, 1);
    }

    #[tokio::test]
    async fn test_handle_offer_nonexistent_session() {
        let (manager, _rx) = create_test_manager();

        let result = manager.handle_offer("nonexistent", "v=0\r\n").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_add_ice_candidate_nonexistent_session() {
        let (manager, _rx) = create_test_manager();

        let result = manager
            .add_ice_candidate("nonexistent", "candidate:...", None, None)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_send_data_nonexistent_session() {
        let (manager, _rx) = create_test_manager();

        let result = manager
            .send_data("nonexistent", "channel", "data", false)
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_stress_many_sessions() {
        let (manager, _rx) = create_test_manager();
        let manager = Arc::new(manager);

        // Create 50 sessions concurrently
        let mut handles = vec![];
        for i in 1..=50 {
            let manager_clone = manager.clone();
            let handle = tokio::spawn(async move {
                manager_clone
                    .create_session(format!("stress-{}", i))
                    .await
            });
            handles.push(handle);
        }

        let results: Vec<_> = futures::future::join_all(handles).await;

        let success_count = results
            .into_iter()
            .filter(|r| r.is_ok() && r.as_ref().unwrap().is_ok())
            .count();

        assert_eq!(success_count, 50, "All 50 sessions should be created");
        assert_eq!(manager.session_count().await, 50);

        // Now close all 50 concurrently
        let mut close_handles = vec![];
        for i in 1..=50 {
            let manager_clone = manager.clone();
            let handle = tokio::spawn(async move {
                manager_clone
                    .close_session(&format!("stress-{}", i))
                    .await
            });
            close_handles.push(handle);
        }

        let close_results: Vec<_> = futures::future::join_all(close_handles).await;

        let close_success = close_results
            .into_iter()
            .filter(|r| r.is_ok() && r.as_ref().unwrap().is_ok())
            .count();

        assert_eq!(close_success, 50, "All 50 sessions should be closed");
        assert_eq!(manager.session_count().await, 0);
    }
}
