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

use crate::adi_frame;
use crate::adi_router::{AdiCallerContext, AdiDiscovery, AdiRouter, AdiRouterBinaryResult};
use crate::filesystem::{FileSystemRequest, handle_request as handle_fs_request};
use crate::protocol::messages::CocoonMessage;
use crate::protocol::types::SilkStream;
use crate::silk::{AnsiToHtml, SilkSession};
use lib_signaling_protocol::SignalingMessage;
use portable_pty::PtySize;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_credential_type::RTCIceCredentialType;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

use lib_env_parse::{env_vars, env_opt};

env_vars! {
    WebrtcIceServers => "WEBRTC_ICE_SERVERS",
    WebrtcTurnUsername => "WEBRTC_TURN_USERNAME",
    WebrtcTurnCredential => "WEBRTC_TURN_CREDENTIAL",
}

fn build_ice_servers() -> Vec<RTCIceServer> {
    let ice_servers_env = env_opt(EnvVar::WebrtcIceServers.as_str());
    let turn_username = env_opt(EnvVar::WebrtcTurnUsername.as_str());
    let turn_credential = env_opt(EnvVar::WebrtcTurnCredential.as_str());

    let urls: Vec<String> = ice_servers_env
        .as_ref()
        .map(|s| s.split(',').map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).collect())
        .unwrap_or_default();

    if urls.is_empty() {
        tracing::info!("No WEBRTC_ICE_SERVERS configured, using default Google STUN server");
        return vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            ..Default::default()
        }];
    }

    let stun_urls: Vec<String> = urls.iter().filter(|u| u.starts_with("stun:")).cloned().collect();
    let turn_urls: Vec<String> = urls.iter().filter(|u| u.starts_with("turn:") || u.starts_with("turns:")).cloned().collect();

    let mut ice_servers = Vec::new();

    if !stun_urls.is_empty() {
        tracing::info!("Configured {} STUN server(s): {:?}", stun_urls.len(), stun_urls);
        ice_servers.push(RTCIceServer {
            urls: stun_urls,
            ..Default::default()
        });
    }

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
            credential_type: RTCIceCredentialType::Password,
        });
    }

    if ice_servers.is_empty() {
        tracing::warn!("No valid ICE servers found, falling back to default Google STUN");
        ice_servers.push(RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            ..Default::default()
        });
    }

    ice_servers
}

struct SilkPtySession {
    id: Uuid,
    pair: portable_pty::PtyPair,
    #[allow(dead_code)]
    child: Box<dyn portable_pty::Child + Send>,
    writer: Box<dyn std::io::Write + Send>,
}

struct SilkDcState {
    silk_sessions: Mutex<HashMap<String, SilkSession>>,
    pty_sessions: Mutex<HashMap<String, SilkPtySession>>,
}

impl SilkDcState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            silk_sessions: Mutex::new(HashMap::new()),
            pty_sessions: Mutex::new(HashMap::new()),
        })
    }
}

pub struct WebRtcSession {
    pub session_id: String,
    pub peer_connection: Arc<RTCPeerConnection>,
    pub data_channels: HashMap<String, Arc<RTCDataChannel>>,
    pub state: String,
    pub user_id: Option<String>,
}

pub struct WebRtcManager {
    sessions: Arc<Mutex<HashMap<String, WebRtcSession>>>,
    signaling_tx: mpsc::UnboundedSender<SignalingMessage>,
    close_timeout: std::time::Duration,
    adi_router: Option<Arc<Mutex<AdiRouter>>>,
}

impl WebRtcManager {
    pub fn new(signaling_tx: mpsc::UnboundedSender<SignalingMessage>) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            signaling_tx,
            close_timeout: std::time::Duration::from_secs(5),
            adi_router: None,
        }
    }

    pub fn with_adi_router(
        signaling_tx: mpsc::UnboundedSender<SignalingMessage>,
        adi_router: Arc<Mutex<AdiRouter>>,
    ) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            signaling_tx,
            close_timeout: std::time::Duration::from_secs(5),
            adi_router: Some(adi_router),
        }
    }

    #[cfg(test)]
    pub fn with_close_timeout(
        signaling_tx: mpsc::UnboundedSender<SignalingMessage>,
        close_timeout: std::time::Duration,
    ) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            signaling_tx,
            close_timeout,
            adi_router: None,
        }
    }

    pub async fn create_session(&self, session_id: String, user_id: Option<String>) -> Result<(), String> {
        tracing::info!("🔧 [create_session] START session_id={}", session_id);
        tracing::info!("🔧 [create_session] current session count: {}", self.sessions.lock().await.len());

        let ice_servers = build_ice_servers();
        tracing::info!("🔧 [create_session] ICE servers configured: {}", ice_servers.len());
        let config = RTCConfiguration {
            ice_servers,
            ..Default::default()
        };

        let mut media_engine = MediaEngine::default();

        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine)
            .map_err(|e| format!("Failed to register interceptors: {}", e))?;

        let setting_engine = SettingEngine::default();
        tracing::info!("🔧 [create_session] SettingEngine created (default, no detach_data_channels)");

        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .with_setting_engine(setting_engine)
            .build();

        tracing::info!("🔧 [create_session] API built, creating peer connection...");

        let peer_connection = api
            .new_peer_connection(config)
            .await
            .map_err(|e| format!("Failed to create peer connection: {}", e))?;

        tracing::info!("🔧 [create_session] peer connection created successfully");
        let peer_connection = Arc::new(peer_connection);

        let session_id_clone = session_id.clone();
        let signaling_tx_clone = self.signaling_tx.clone();
        peer_connection.on_ice_candidate(Box::new(move |candidate| {
            let session_id = session_id_clone.clone();
            let tx = signaling_tx_clone.clone();

            Box::pin(async move {
                if let Some(c) = candidate {
                    if let Ok(json) = c.to_json() {
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

                        // webrtc-rs hardcodes sdp_mid="" for gathered candidates but browsers
                        // reject candidates where sdpMid doesn't match the SDP media section id.
                        // The data channel is always media section "0", so we fix it here.
                        let sdp_mid = match json.sdp_mid.as_deref() {
                            Some("") | None => Some("0".to_string()),
                            other => other.map(|s| s.to_string()),
                        };

                        tracing::debug!(
                            "🧊 ICE candidate gathered for session {}: type={}, mid={:?}",
                            session_id,
                            candidate_type,
                            sdp_mid
                        );

                        let _ = tx.send(SignalingMessage::SyncData {
                            payload: serde_json::to_value(&CocoonMessage::WebrtcIceCandidate {
                                session_id,
                                candidate: json.candidate,
                                sdp_mid,
                                sdp_mline_index: json.sdp_mline_index.map(|i| i as i32),
                            }).unwrap(),
                        });
                    }
                } else {
                    tracing::debug!("🧊 ICE gathering complete for session {}", session_id);
                }
            })
        }));

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

        let session_id_clone = session_id.clone();
        peer_connection.on_ice_connection_state_change(Box::new(move |state| {
            let session_id = session_id_clone.clone();
            Box::pin(async move {
                tracing::warn!(
                    "🧊 [ICE-CONN] session={} state={:?}",
                    session_id,
                    state
                );
            })
        }));

        let session_id_clone = session_id.clone();
        let signaling_tx_clone = self.signaling_tx.clone();
        let sessions_clone = self.sessions.clone();
        peer_connection.on_peer_connection_state_change(Box::new(move |state| {
            let session_id = session_id_clone.clone();
            let tx = signaling_tx_clone.clone();
            let sessions = sessions_clone.clone();

            Box::pin(async move {
                tracing::warn!("🔌 [PC-STATE] session={} state={:?}", session_id, state);

                match state {
                    RTCPeerConnectionState::New => {
                        tracing::info!("🔌 [PC-STATE] session={} → New", session_id);
                    }
                    RTCPeerConnectionState::Connecting => {
                        tracing::info!("🔌 [PC-STATE] session={} → Connecting (DTLS handshake starting)", session_id);
                    }
                    RTCPeerConnectionState::Connected => {
                        tracing::info!("✅ [PC-STATE] session={} → Connected! WebRTC fully established", session_id);
                        if let Some(session) = sessions.lock().await.get_mut(&session_id) {
                            session.state = "connected".to_string();
                        }
                    }
                    RTCPeerConnectionState::Disconnected
                    | RTCPeerConnectionState::Failed
                    | RTCPeerConnectionState::Closed => {
                        let reason = match state {
                            RTCPeerConnectionState::Disconnected => {
                                tracing::warn!("⚠️ [PC-STATE] session={} → Disconnected (ICE lost connectivity, may recover)", session_id);
                                "disconnected"
                            }
                            RTCPeerConnectionState::Failed => {
                                tracing::error!(
                                    "❌ [PC-STATE] session={} → Failed! ICE connectivity could not be established. \
                                    Check WEBRTC_ICE_SERVERS config and ensure TURN server is available for NAT traversal.",
                                    session_id
                                );
                                "failed"
                            }
                            RTCPeerConnectionState::Closed => {
                                tracing::info!("🔌 [PC-STATE] session={} → Closed (normal shutdown)", session_id);
                                "closed"
                            }
                            _ => "unknown",
                        };

                        let _ = tx.send(SignalingMessage::SyncData {
                            payload: serde_json::to_value(&CocoonMessage::WebrtcSessionEnded {
                                session_id: session_id.clone(),
                                reason: Some(reason.to_string()),
                            }).unwrap(),
                        });

                        sessions.lock().await.remove(&session_id);
                    }
                    _ => {
                        tracing::info!("🔌 [PC-STATE] session={} → unhandled state {:?}", session_id, state);
                    }
                }
            })
        }));

        // Per-session silk state (outlives individual data channel handler calls)
        let silk_state = SilkDcState::new();

        let session_id_clone = session_id.clone();
        let signaling_tx_clone = self.signaling_tx.clone();
        let sessions_clone = self.sessions.clone();
        let adi_router_clone = self.adi_router.clone();
        let user_id_clone = user_id.clone();
        let silk_state_clone = silk_state.clone();
        peer_connection.on_data_channel(Box::new(move |dc| {
            let session_id = session_id_clone.clone();
            let tx = signaling_tx_clone.clone();
            let sessions = sessions_clone.clone();
            let dc_label = dc.label().to_string();
            let adi_router = adi_router_clone.clone();
            let user_id = user_id_clone.clone();
            let silk_state = silk_state_clone.clone();

            Box::pin(async move {
                tracing::warn!(
                    "📡 [DATA-CHANNEL] on_data_channel FIRED! session={} label={} id={} readyState={:?}",
                    session_id,
                    dc_label,
                    dc.id(),
                    dc.ready_state(),
                );

                if let Some(session) = sessions.lock().await.get_mut(&session_id) {
                    session.data_channels.insert(dc_label.clone(), dc.clone());
                }

                let dc_label_clone = dc_label.clone();
                let session_id_clone = session_id.clone();
                let tx_clone = tx.clone();
                let dc_clone = dc.clone();
                let adi_router_for_msg = adi_router.clone();
                let user_id_for_msg = user_id.clone();
                let silk_state_for_msg = silk_state.clone();
                dc.on_message(Box::new(move |msg: DataChannelMessage| {
                    let session_id = session_id_clone.clone();
                    let channel = dc_label_clone.clone();
                    let tx = tx_clone.clone();
                    let dc_for_response = dc_clone.clone();
                    let adi_router = adi_router_for_msg.clone();
                    let user_id = user_id_for_msg.clone();
                    let silk_state = silk_state_for_msg.clone();

                    Box::pin(async move {
                        tracing::warn!(
                            "📨 [DC-MSG] on_message FIRED! session={} channel={} len={} is_string={}",
                            session_id, channel, msg.data.len(), msg.is_string
                        );

                        if channel == "adi" && !msg.is_string {
                            if let Some(router) = &adi_router {
                                tracing::debug!("📦 ADI binary request received: {} bytes", msg.data.len());

                                let ctx = AdiCallerContext {
                                    user_id: user_id.clone(),
                                    device_id: None,
                                };

                                let router_guard = router.lock().await;
                                let result = router_guard.handle_binary(&ctx, &msg.data).await;
                                drop(router_guard);

                                match result {
                                    AdiRouterBinaryResult::Single(response_bytes) => {
                                        let len = response_bytes.len();
                                        if let Err(e) = dc_for_response.send(&response_bytes.into()).await {
                                            tracing::error!("❌ Failed to send ADI binary response: {}", e);
                                        } else {
                                            tracing::debug!("📤 ADI binary response sent: {} bytes", len);
                                        }
                                    }
                                    AdiRouterBinaryResult::Stream { request_id, mut receiver } => {
                                        let dc_for_stream = dc_for_response.clone();
                                        tokio::spawn(async move {
                                            let mut seq = 0u32;
                                            while let Some((chunk_data, is_final)) = receiver.recv().await {
                                                let frame = if is_final {
                                                    adi_frame::stream_end(request_id, seq, &chunk_data)
                                                } else {
                                                    adi_frame::stream_chunk(request_id, seq, &chunk_data)
                                                };
                                                seq += 1;

                                                if let Err(e) = dc_for_stream.send(&frame.into()).await {
                                                    tracing::error!("❌ Failed to send ADI stream chunk: {}", e);
                                                    break;
                                                }

                                                if is_final {
                                                    break;
                                                }
                                            }
                                        });
                                    }
                                }
                            } else {
                                tracing::warn!("⚠️ ADI binary request received but no router configured");
                            }
                            return;
                        }

                        let (data, binary) = if msg.is_string {
                            (String::from_utf8_lossy(&msg.data).to_string(), false)
                        } else {
                            (base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &msg.data), true)
                        };

                        if channel == "silk" {
                            tracing::info!("🧵 [DC-MSG] Silk message received: {} bytes, preview={}", data.len(), &data[..data.len().min(200)]);
                            match serde_json::from_str::<CocoonMessage>(&data) {
                                Ok(cocoon_msg) => {
                                    let dc = dc_for_response.clone();
                                    tokio::spawn(async move {
                                        handle_silk_dc_msg(cocoon_msg, silk_state, dc).await;
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!("⚠️ Invalid silk message: {}", e);
                                }
                            }
                            return;
                        }

                        if channel == "file" {
                            tracing::debug!("📁 File system request received: {} bytes", data.len());
                            match serde_json::from_str::<FileSystemRequest>(&data) {
                                Ok(request) => {
                                    let response = handle_fs_request(request).await;
                                    match serde_json::to_string(&response) {
                                        Ok(response_json) => {
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

                        if channel == "adi" {
                            if let Some(router) = &adi_router {
                                if let Ok(discovery) = serde_json::from_str::<AdiDiscovery>(&data) {
                                    let router_guard = router.lock().await;
                                    let response = router_guard.handle_discovery(discovery);
                                    drop(router_guard);

                                    if let Ok(response_json) = serde_json::to_string(&response) {
                                        if let Err(e) = dc_for_response.send(&response_json.into_bytes().into()).await {
                                            tracing::error!("❌ Failed to send ADI discovery response: {}", e);
                                        }
                                    }
                                    return;
                                }

                                // Text frames on "adi" that aren't discovery are ignored
                                // (all service requests use binary framing now)
                                tracing::warn!("⚠️ Unrecognized text message on adi channel: {}",
                                    &data[..data.len().min(200)]);
                            } else {
                                tracing::warn!("⚠️ ADI request received but no router configured");
                                let error_response = serde_json::json!({
                                    "type": "error",
                                    "request_id": null,
                                    "plugin": "",
                                    "method": "",
                                    "code": "no_router",
                                    "message": "ADI plugin router not configured"
                                });
                                if let Ok(error_json) = serde_json::to_string(&error_response) {
                                    let _ = dc_for_response.send(&error_json.into_bytes().into()).await;
                                }
                            }
                            return;
                        }

                        let _ = tx.send(SignalingMessage::SyncData {
                            payload: serde_json::to_value(&CocoonMessage::WebrtcData {
                                session_id,
                                channel,
                                data,
                                binary,
                            }).unwrap(),
                        });
                    })
                }));
            })
        }));

        // Store the session (silk_state is held alive by the on_data_channel closure)
        drop(silk_state);
        let session = WebRtcSession {
            session_id: session_id.clone(),
            peer_connection,
            data_channels: HashMap::new(),
            state: "pending".to_string(),
            user_id,
        };

        self.sessions.lock().await.insert(session_id.clone(), session);
        tracing::info!("🔧 [create_session] END session_id={} — stored and ready for offer", session_id);

        Ok(())
    }

    pub async fn handle_offer(&self, session_id: &str, sdp: &str) -> Result<String, String> {
        tracing::info!("📥 [handle_offer] START session_id={} sdp_len={}", session_id, sdp.len());

        // Clone the peer_connection Arc and drop the lock BEFORE async WebRTC ops.
        // set_remote_description can trigger on_data_channel which also locks sessions — holding
        // the lock across these calls would deadlock.
        let pc = {
            let sessions = self.sessions.lock().await;
            tracing::info!("📥 [handle_offer] lock acquired, sessions_count={}", sessions.len());

            let session = sessions
                .get(session_id)
                .ok_or_else(|| {
                    let keys: Vec<_> = sessions.keys().collect();
                    tracing::error!("📥 [handle_offer] session NOT FOUND! id={} available={:?}", session_id, keys);
                    format!("Session {} not found", session_id)
                })?;

            tracing::info!("📥 [handle_offer] session found, state={}", session.state);
            session.peer_connection.clone()
            // lock dropped here
        };

        let offer = RTCSessionDescription::offer(sdp.to_string())
            .map_err(|e| format!("Failed to parse SDP offer: {}", e))?;
        tracing::info!("📥 [handle_offer] SDP offer parsed successfully");

        // Set remote description (may trigger on_data_channel — lock must NOT be held)
        tracing::info!("📥 [handle_offer] setting remote description...");
        pc.set_remote_description(offer)
            .await
            .map_err(|e| format!("Failed to set remote description: {}", e))?;
        tracing::info!("📥 [handle_offer] remote description set OK");

        tracing::info!("📥 [handle_offer] creating answer...");
        let answer = pc
            .create_answer(None)
            .await
            .map_err(|e| format!("Failed to create answer: {}", e))?;
        tracing::info!("📥 [handle_offer] answer created, sdp_len={}", answer.sdp.len());

        tracing::info!("📥 [handle_offer] setting local description...");
        pc.set_local_description(answer.clone())
            .await
            .map_err(|e| format!("Failed to set local description: {}", e))?;
        tracing::info!("📥 [handle_offer] local description set OK — answer ready to send");

        Ok(answer.sdp)
    }

    pub async fn add_ice_candidate(
        &self,
        session_id: &str,
        candidate: &str,
        sdp_mid: Option<&str>,
        sdp_mline_index: Option<u32>,
    ) -> Result<(), String> {
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
            "🧊 Remote ICE candidate received for session {}: type={}, mid={:?}, candidate={}",
            session_id,
            candidate_type,
            sdp_mid,
            candidate
        );

        let pc = {
            let sessions = self.sessions.lock().await;
            let session = sessions
                .get(session_id)
                .ok_or_else(|| {
                    tracing::error!("🧊 [add_ice] session NOT FOUND: {}", session_id);
                    format!("Session {} not found", session_id)
                })?;
            session.peer_connection.clone()
        };

        let ice_candidate = webrtc::ice_transport::ice_candidate::RTCIceCandidateInit {
            candidate: candidate.to_string(),
            sdp_mid: sdp_mid.map(|s| s.to_string()),
            sdp_mline_index: sdp_mline_index.map(|i| i as u16),
            ..Default::default()
        };

        tracing::info!("🧊 [add_ice] adding candidate to PC for session={}", session_id);
        pc.add_ice_candidate(ice_candidate)
            .await
            .map_err(|e| {
                tracing::error!("🧊 [add_ice] FAILED for session={}: {}", session_id, e);
                format!("Failed to add ICE candidate: {}", e)
            })?;
        tracing::info!("🧊 [add_ice] candidate added OK for session={}", session_id);

        Ok(())
    }

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

    pub async fn list_sessions(&self) -> Vec<String> {
        self.sessions
            .lock()
            .await
            .keys()
            .cloned()
            .collect()
    }

    pub async fn session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }

    pub async fn session_exists(&self, session_id: &str) -> bool {
        self.sessions.lock().await.contains_key(session_id)
    }

    pub async fn get_session_state(&self, session_id: &str) -> Option<String> {
        self.sessions
            .lock()
            .await
            .get(session_id)
            .map(|s| s.state.clone())
    }
}

async fn dc_send(dc: &RTCDataChannel, msg: &CocoonMessage) {
    match serde_json::to_string(msg) {
        Ok(json) => {
            tracing::warn!("📤 [dc_send] sending {} bytes, dc_id={}, readyState={:?}, preview={}", json.len(), dc.id(), dc.ready_state(), &json[..json.len().min(200)]);
            match dc.send(&json.into_bytes().into()).await {
                Ok(n) => {
                    tracing::warn!("📤 [dc_send] OK — sent {} bytes", n);
                }
                Err(e) => {
                    tracing::error!("📤 [dc_send] FAILED: {}", e);
                }
            }
        }
        Err(e) => {
            tracing::error!("📤 [dc_send] serialization FAILED: {}", e);
        }
    }
}

async fn handle_silk_dc_msg(
    msg: CocoonMessage,
    state: Arc<SilkDcState>,
    dc: Arc<RTCDataChannel>,
) {
    match msg {
        CocoonMessage::SilkCreateSession { cwd, env, shell } => {
            tracing::warn!("🧵 [SILK] Creating session cwd={:?} shell={:?}", cwd, shell);
            let env = env.unwrap_or_default();
            tracing::warn!("🧵 [SILK] Calling SilkSession::new...");
            match SilkSession::new(cwd, env, shell) {
                Ok(session) => {
                    tracing::warn!("🧵 [SILK] Session OK id={} cwd={} shell={}", session.id, session.cwd, session.shell);
                    let response = CocoonMessage::SilkCreateSessionResponse {
                        session_id: session.id.to_string(),
                        cwd: session.cwd.clone(),
                        shell: session.shell.clone(),
                    };
                    tracing::warn!("🧵 [SILK] Acquiring silk_sessions lock...");
                    state.silk_sessions.lock().await.insert(session.id.to_string(), session);
                    tracing::warn!("🧵 [SILK] Session stored, calling dc_send...");
                    dc_send(&dc, &response).await;
                    tracing::warn!("🧵 [SILK] dc_send COMPLETE — response sent!");
                }
                Err(e) => {
                    tracing::error!("🧵 [SILK] SilkSession::new FAILED: {}", e);
                    dc_send(&dc, &CocoonMessage::SilkError {
                        session_id: None,
                        command_id: None,
                        code: "session_create_failed".to_string(),
                        message: e,
                    }).await;
                    tracing::warn!("🧵 [SILK] Error response sent");
                }
            }
        }

        CocoonMessage::SilkExecute { session_id, command, command_id, cols, rows, .. } => {
            tracing::info!("🧵 [DC] Silk execute: {} (session {})", command, session_id);
            let mut sessions = state.silk_sessions.lock().await;
            let Some(session) = sessions.get_mut(&session_id) else {
                drop(sessions);
                dc_send(&dc, &CocoonMessage::SilkError {
                    session_id: Some(session_id),
                    command_id: Some(command_id),
                    code: "session_not_found".to_string(),
                    message: "Silk session not found".to_string(),
                }).await;
                return;
            };

            match session.execute(&command, command_id.clone()) {
                Ok((interactive, child_opt)) => {
                    if interactive {
                        drop(sessions);
                        let dc_for_pty = dc.clone();
                        let state_for_pty = state.clone();
                        let term_cols = cols.map(|c| c as u16).unwrap_or(80);
                        let term_rows = rows.map(|r| r as u16).unwrap_or(24);
                        let pty_id = Uuid::new_v4();

                        let pty_system = portable_pty::native_pty_system();
                        match pty_system.openpty(PtySize { rows: term_rows, cols: term_cols, pixel_width: 0, pixel_height: 0 }) {
                            Ok(pair) => {
                                let mut cmd = portable_pty::CommandBuilder::new("/bin/sh");
                                cmd.arg("-c");
                                cmd.arg(&command);
                                cmd.env("TERM", "xterm-256color");

                                match pair.slave.spawn_command(cmd) {
                                    Ok(child) => {
                                        if let Some(s) = state_for_pty.silk_sessions.lock().await.get_mut(&session_id) {
                                            s.set_pty_session(command_id.clone(), pty_id);
                                        }

                                        let mut reader = pair.master.try_clone_reader().unwrap();
                                        let session_id_for_pty = session_id.clone();
                                        let command_id_for_pty = command_id.clone();
                                        let pty_id_str = pty_id.to_string();
                                        tokio::task::spawn_blocking(move || {
                                            let mut buf = [0u8; 4096];
                                            loop {
                                                match reader.read(&mut buf) {
                                                    Ok(0) => break,
                                                    Ok(n) => {
                                                        let data = String::from_utf8_lossy(&buf[..n]).to_string();
                                                        let response = CocoonMessage::SilkPtyOutput {
                                                            session_id: session_id_for_pty.clone(),
                                                            command_id: command_id_for_pty.clone(),
                                                            pty_session_id: pty_id_str.clone(),
                                                            data,
                                                        };
                                                        let dc_clone = dc_for_pty.clone();
                                                        tokio::spawn(async move {
                                                            dc_send(&dc_clone, &response).await;
                                                        });
                                                    }
                                                    Err(_) => break,
                                                }
                                            }
                                        });

                                        let pty_writer = pair.master.take_writer().unwrap();
                                        let pty_session = SilkPtySession {
                                            id: pty_id,
                                            pair,
                                            child,
                                            writer: pty_writer,
                                        };
                                        state_for_pty.pty_sessions.lock().await.insert(command_id.clone(), pty_session);

                                        dc_send(&dc, &CocoonMessage::SilkInteractiveRequired {
                                            session_id,
                                            command_id,
                                            reason: format!("Command '{}' requires interactive mode", command.split_whitespace().next().unwrap_or(&command)),
                                            pty_session_id: pty_id.to_string(),
                                        }).await;
                                    }
                                    Err(e) => {
                                        dc_send(&dc, &CocoonMessage::SilkError {
                                            session_id: Some(session_id),
                                            command_id: Some(command_id),
                                            code: "pty_spawn_failed".to_string(),
                                            message: e.to_string(),
                                        }).await;
                                    }
                                }
                            }
                            Err(e) => {
                                dc_send(&dc, &CocoonMessage::SilkError {
                                    session_id: Some(session_id),
                                    command_id: Some(command_id),
                                    code: "pty_create_failed".to_string(),
                                    message: e.to_string(),
                                }).await;
                            }
                        }
                    } else if let Some(mut child) = child_opt {
                        drop(sessions);
                        let dc_for_out = dc.clone();
                        let state_for_out = state.clone();
                        let command_id_clone = command_id.clone();

                        dc_send(&dc, &CocoonMessage::SilkCommandStarted {
                            session_id: session_id.clone(),
                            command_id: command_id.clone(),
                            interactive: false,
                        }).await;

                        tokio::spawn(async move {
                            let command_id = command_id_clone;
                            let mut stdout = std::io::BufReader::new(child.stdout.take().expect("stdout piped"));
                            let mut stderr = std::io::BufReader::new(child.stderr.take().expect("stderr piped"));
                            let mut buf = [0u8; 4096];

                            loop {
                                match stdout.get_mut().read(&mut buf) {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        let data = String::from_utf8_lossy(&buf[..n]).to_string();
                                        let html = AnsiToHtml::convert(&data);
                                        dc_send(&dc_for_out, &CocoonMessage::SilkOutput {
                                            session_id: session_id.clone(),
                                            command_id: command_id.clone(),
                                            stream: SilkStream::Stdout,
                                            data,
                                            html: Some(html),
                                        }).await;
                                    }
                                    Err(_) => break,
                                }
                            }

                            let mut stderr_buf = Vec::new();
                            let _ = stderr.read_to_end(&mut stderr_buf);
                            if !stderr_buf.is_empty() {
                                let data = String::from_utf8_lossy(&stderr_buf).to_string();
                                let html = AnsiToHtml::convert(&data);
                                dc_send(&dc_for_out, &CocoonMessage::SilkOutput {
                                    session_id: session_id.clone(),
                                    command_id: command_id.clone(),
                                    stream: SilkStream::Stderr,
                                    data,
                                    html: Some(html),
                                }).await;
                            }

                            let exit_code = child.wait().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);

                            let mut sessions = state_for_out.silk_sessions.lock().await;
                            let cwd = if let Some(s) = sessions.get_mut(&session_id) {
                                s.update_cwd_if_cd(&command);
                                s.complete_command(command_id.clone());
                                s.cwd.clone()
                            } else {
                                String::new()
                            };
                            drop(sessions);

                            dc_send(&dc_for_out, &CocoonMessage::SilkCommandCompleted {
                                session_id,
                                command_id,
                                exit_code,
                                cwd,
                            }).await;
                        });
                    } else {
                        dc_send(&dc, &CocoonMessage::SilkError {
                            session_id: Some(session_id),
                            command_id: Some(command_id),
                            code: "execute_failed".to_string(),
                            message: "No child process".to_string(),
                        }).await;
                    }
                }
                Err(e) => {
                    dc_send(&dc, &CocoonMessage::SilkError {
                        session_id: Some(session_id),
                        command_id: Some(command_id),
                        code: "execute_failed".to_string(),
                        message: e,
                    }).await;
                }
            }
        }

        CocoonMessage::SilkInput { session_id, command_id, data } => {
            let mut pty_sessions = state.pty_sessions.lock().await;
            if let Some(pty) = pty_sessions.get_mut(&command_id) {
                if let Err(e) = std::io::Write::write_all(&mut pty.writer, data.as_bytes()) {
                    tracing::warn!("⚠️ PTY write failed for {}: {}", command_id, e);
                } else {
                    let _ = std::io::Write::flush(&mut pty.writer);
                }
            } else {
                dc_send(&dc, &CocoonMessage::SilkError {
                    session_id: Some(session_id),
                    command_id: Some(command_id),
                    code: "command_not_found".to_string(),
                    message: "Interactive command not found".to_string(),
                }).await;
            }
        }

        CocoonMessage::SilkResize { session_id: _, command_id, cols, rows } => {
            let pty_sessions = state.pty_sessions.lock().await;
            if let Some(pty) = pty_sessions.get(&command_id) {
                let _ = pty.pair.master.resize(PtySize {
                    rows: rows as u16,
                    cols: cols as u16,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
        }

        CocoonMessage::SilkCloseSession { session_id } => {
            tracing::info!("🧵 [DC] Closing silk session {}", session_id);
            state.silk_sessions.lock().await.remove(&session_id);
            dc_send(&dc, &CocoonMessage::SilkSessionClosed { session_id }).await;
        }

        _ => {
            tracing::debug!("🧵 [DC] Unhandled silk message type");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn create_test_manager() -> (WebRtcManager, mpsc::UnboundedReceiver<SignalingMessage>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let manager =
            WebRtcManager::with_close_timeout(tx, std::time::Duration::from_millis(100));
        (manager, rx)
    }

    #[tokio::test]
    async fn test_create_single_session() {
        let (manager, _rx) = create_test_manager();

        let result = manager.create_session("session-1".to_string(), None).await;
        assert!(result.is_ok(), "Failed to create session: {:?}", result);

        assert!(manager.session_exists("session-1").await);
        assert_eq!(manager.session_count().await, 1);
    }

    #[tokio::test]
    async fn test_create_multiple_sessions_sequentially() {
        let (manager, _rx) = create_test_manager();

        for i in 1..=5 {
            let session_id = format!("session-{}", i);
            let result = manager.create_session(session_id.clone(), None).await;
            assert!(
                result.is_ok(),
                "Failed to create session {}: {:?}",
                i,
                result
            );
            assert!(manager.session_exists(&session_id).await);
        }

        assert_eq!(manager.session_count().await, 5);

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

        let mut handles = vec![];
        for i in 1..=10 {
            let manager_clone = manager.clone();
            let handle = tokio::spawn(async move {
                let session_id = format!("concurrent-session-{}", i);
                manager_clone.create_session(session_id, None).await
            });
            handles.push(handle);
        }

        let results: Vec<_> = futures::future::join_all(handles).await;
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

        manager
            .create_session("session-to-close".to_string(), None)
            .await
            .expect("Failed to create session");
        assert!(manager.session_exists("session-to-close").await);

        manager
            .close_session("session-to-close")
            .await
            .expect("Failed to close session");

        assert!(!manager.session_exists("session-to-close").await);
        assert_eq!(manager.session_count().await, 0);
    }

    #[tokio::test]
    async fn test_recreate_session_after_close() {
        let (manager, _rx) = create_test_manager();

        manager
            .create_session("recyclable-session".to_string(), None)
            .await
            .expect("Failed to create initial session");
        assert!(manager.session_exists("recyclable-session").await);

        manager
            .close_session("recyclable-session")
            .await
            .expect("Failed to close session");
        assert!(!manager.session_exists("recyclable-session").await);

        let result = manager
            .create_session("recyclable-session".to_string(), None)
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

        for cycle in 1..=5 {
            let result = manager
                .create_session("lifecycle-test".to_string(), None)
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

        let result = manager.close_session("nonexistent").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_multiple_sessions_independent_lifecycle() {
        let (manager, _rx) = create_test_manager();

        manager
            .create_session("session-a".to_string(), None)
            .await
            .expect("Failed to create session-a");
        manager
            .create_session("session-b".to_string(), None)
            .await
            .expect("Failed to create session-b");
        manager
            .create_session("session-c".to_string(), None)
            .await
            .expect("Failed to create session-c");

        assert_eq!(manager.session_count().await, 3);

        manager
            .close_session("session-b")
            .await
            .expect("Failed to close session-b");

        assert!(manager.session_exists("session-a").await);
        assert!(!manager.session_exists("session-b").await);
        assert!(manager.session_exists("session-c").await);
        assert_eq!(manager.session_count().await, 2);

        manager
            .create_session("session-b".to_string(), None)
            .await
            .expect("Failed to recreate session-b");

        assert_eq!(manager.session_count().await, 3);
        assert!(manager.session_exists("session-b").await);
    }

    #[tokio::test]
    async fn test_initial_session_state() {
        let (manager, _rx) = create_test_manager();

        manager
            .create_session("state-test".to_string(), None)
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

        for i in 1..=20 {
            let result = manager.create_session(format!("rapid-{}", i), None).await;
            assert!(
                result.is_ok(),
                "Rapid cycle {}: create failed: {:?}",
                i,
                result
            );

            manager
                .close_session(&format!("rapid-{}", i))
                .await
                .expect(&format!("Rapid cycle {}: close failed", i));
        }

        assert_eq!(manager.session_count().await, 0);
    }

    #[tokio::test]
    async fn test_concurrent_create_and_close() {
        let (manager, _rx) = create_test_manager();
        let manager = Arc::new(manager);

        for i in 1..=10 {
            manager
                .create_session(format!("cc-session-{}", i), None)
                .await
                .expect("Failed to create session");
        }

        let mut handles = vec![];

        for i in (1..=10).step_by(2) {
            let manager_clone = manager.clone();
            let handle = tokio::spawn(async move {
                manager_clone
                    .close_session(&format!("cc-session-{}", i))
                    .await
            });
            handles.push(handle);
        }

        for i in 11..=15 {
            let manager_clone = manager.clone();
            let handle = tokio::spawn(async move {
                manager_clone
                    .create_session(format!("cc-session-{}", i), None)
                    .await
            });
            handles.push(handle);
        }

        let results: Vec<_> = futures::future::join_all(handles).await;
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

        manager
            .create_session("duplicate-test".to_string(), None)
            .await
            .expect("Failed to create first session");

        let result = manager.create_session("duplicate-test".to_string(), None).await;
        assert!(result.is_ok(), "Second create should succeed");

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

        let mut handles = vec![];
        for i in 1..=50 {
            let manager_clone = manager.clone();
            let handle = tokio::spawn(async move {
                manager_clone
                    .create_session(format!("stress-{}", i), None)
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
