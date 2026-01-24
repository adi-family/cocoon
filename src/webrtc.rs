//! WebRTC session handling for cocoon
//!
//! Provides WebRTC peer connection management for low-latency, direct
//! communication between browser clients and cocoons.

use lib_tarminal_sync::SignalingMessage;
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
}

impl WebRtcManager {
    /// Create a new WebRTC manager
    pub fn new(signaling_tx: mpsc::UnboundedSender<SignalingMessage>) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            signaling_tx,
        }
    }

    /// Create a new WebRTC peer connection for a session
    pub async fn create_session(&self, session_id: String) -> Result<(), String> {
        let config = RTCConfiguration {
            ice_servers: vec![
                RTCIceServer {
                    urls: vec!["stun:stun.l.google.com:19302".to_string()],
                    ..Default::default()
                },
            ],
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
                        let _ = tx.send(SignalingMessage::WebRtcIceCandidate {
                            session_id,
                            candidate: json.candidate,
                            sdp_mid: json.sdp_mid,
                            sdp_mline_index: json.sdp_mline_index.map(|i| i as u32),
                        });
                    }
                }
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
                        if let Some(session) = sessions.lock().await.get_mut(&session_id) {
                            session.state = "connected".to_string();
                        }
                    }
                    RTCPeerConnectionState::Disconnected
                    | RTCPeerConnectionState::Failed
                    | RTCPeerConnectionState::Closed => {
                        let reason = match state {
                            RTCPeerConnectionState::Disconnected => "disconnected",
                            RTCPeerConnectionState::Failed => "failed",
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
                dc.on_message(Box::new(move |msg: DataChannelMessage| {
                    let session_id = session_id_clone.clone();
                    let channel = dc_label_clone.clone();
                    let tx = tx_clone.clone();

                    Box::pin(async move {
                        let (data, binary) = if msg.is_string {
                            (String::from_utf8_lossy(&msg.data).to_string(), false)
                        } else {
                            (base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &msg.data), true)
                        };

                        // Forward through signaling (for processing)
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

    /// Add an ICE candidate
    pub async fn add_ice_candidate(
        &self,
        session_id: &str,
        candidate: &str,
        sdp_mid: Option<&str>,
        sdp_mline_index: Option<u32>,
    ) -> Result<(), String> {
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
    pub async fn close_session(&self, session_id: &str) -> Result<(), String> {
        if let Some(session) = self.sessions.lock().await.remove(session_id) {
            session
                .peer_connection
                .close()
                .await
                .map_err(|e| format!("Failed to close peer connection: {}", e))?;
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
}
