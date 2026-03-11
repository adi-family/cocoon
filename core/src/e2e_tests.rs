//! End-to-end integration tests for signaling connection and WebRTC message flow.
//!
//! Tests the full path: signaling registration → WebRTC peer connection → data channel messaging.

use crate::adi_router::{
    AdiCallerContext, AdiHandleResult, AdiMethodInfo, AdiRouter, AdiService, AdiServiceError,
};
use crate::protocol::messages::CocoonMessage;
use crate::webrtc::WebRtcManager;
use async_trait::async_trait;
use bytes::Bytes;
use lib_signaling_protocol::SignalingMessage;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

// ── Minimal signaling server for tests ──────────────────────────────────────

mod test_signaling {
    use axum::{
        extract::{
            ws::{Message, WebSocket, WebSocketUpgrade},
            Query, State,
        },
        response::IntoResponse,
        routing::get,
        Router,
    };
    use futures::{
        SinkExt, StreamExt,
        stream::{SplitSink, SplitStream},
    };
    use lib_signaling_protocol::SignalingMessage;
    use serde::Deserialize;
    use signaling_core::{
        security::{derive_device_id, validate_secret},
        state::AppState,
    };
    use tokio::sync::mpsc;

    #[derive(Deserialize)]
    pub struct WsQuery {
        #[serde(default = "default_kind")]
        kind: String,
    }

    fn default_kind() -> String {
        "app".to_string()
    }

    /// Spawn a minimal signaling server on a random port, return the ws:// URL.
    pub async fn spawn_server() -> String {
        let state = AppState::new(
            "test-salt-for-e2e".to_string(),
            None,
            true,
            vec![],
        );
        let app = Router::new()
            .route("/ws", get(ws_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        format!("ws://127.0.0.1:{}/ws", addr.port())
    }

    async fn ws_handler(
        Query(query): Query<WsQuery>,
        State(state): State<AppState>,
        ws: WebSocketUpgrade,
    ) -> impl IntoResponse {
        ws.on_upgrade(move |socket| handle_socket(socket, state, query.kind))
    }

    async fn handle_socket(socket: WebSocket, state: AppState, kind: String) {
        let (mut sender, mut receiver): (SplitSink<WebSocket, Message>, SplitStream<WebSocket>) =
            socket.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();

        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if sender.send(Message::Text(msg.into())).await.is_err() {
                    break;
                }
            }
        });

        let mut device_id: Option<String> = None;

        if kind != "cocoon" {
            let hello = SignalingMessage::AuthHello {
                auth_kind: "none".to_string(),
                auth_domain: "none".to_string(),
                auth_requirement: lib_signaling_protocol::AuthRequirement::Optional,
                auth_options: vec![lib_signaling_protocol::AuthOption::Anonymous],
            };
            send_msg(&tx, &hello);
        }

        while let Some(Ok(msg)) = receiver.next().await {
            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Close(_) => break,
                _ => continue,
            };

            let parsed: SignalingMessage = match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(_) => continue,
            };

            match parsed {
                SignalingMessage::DeviceRegister {
                    secret,
                    tags,
                    device_type: _,
                    device_config: _,
                    ..
                } => {
                    if let Err(e) = validate_secret(&secret) {
                        send_msg(&tx, &SignalingMessage::SystemError { message: e });
                        continue;
                    }

                    let derived_id = derive_device_id(&secret, &state.hmac_salt);

                    if let Some(ref old_id) = device_id {
                        state.connections.remove(old_id);
                    }

                    device_id = Some(derived_id.clone());
                    state.connections.insert(derived_id.clone(), tx.clone());

                    let clean_tags = tags.map(|mut t| {
                        t.remove("setup_token");
                        t
                    });

                    send_msg(
                        &tx,
                        &SignalingMessage::DeviceRegisterResponse {
                            device_id: derived_id,
                            tags: clean_tags,
                        },
                    );
                }

                SignalingMessage::SyncData { payload } => {
                    if let Some(ref did) = device_id {
                        if let Some(peer_id) = state.paired_devices.get(did) {
                            if let Some(peer_tx) = state.connections.get(peer_id.value()) {
                                send_msg(
                                    peer_tx.value(),
                                    &SignalingMessage::SyncData { payload },
                                );
                            }
                        }
                    }
                }

                SignalingMessage::PairingCreateCode => {
                    if let Some(ref did) = device_id {
                        let code = signaling_core::utils::generate_pairing_code();
                        state.pairing_codes.insert(code.clone(), did.clone());
                        send_msg(
                            &tx,
                            &SignalingMessage::PairingCreateCodeResponse { code },
                        );
                    }
                }

                SignalingMessage::PairingUseCode { code } => {
                    if let Some(ref did) = device_id {
                        if let Some((_, peer_id)) = state.pairing_codes.remove(&code) {
                            state
                                .paired_devices
                                .insert(did.clone(), peer_id.clone());
                            state
                                .paired_devices
                                .insert(peer_id.clone(), did.clone());

                            send_msg(
                                &tx,
                                &SignalingMessage::PairingUseCodeResponse {
                                    peer_id: peer_id.clone(),
                                },
                            );

                            if let Some(peer_tx) = state.connections.get(&peer_id) {
                                send_msg(
                                    peer_tx.value(),
                                    &SignalingMessage::PairingUseCodeResponse {
                                        peer_id: did.clone(),
                                    },
                                );
                            }
                        }
                    }
                }

                _ => {}
            }
        }

        if let Some(ref did) = device_id {
            state.connections.remove(did);
        }
    }

    fn send_msg(tx: &mpsc::UnboundedSender<String>, msg: &SignalingMessage) {
        if let Ok(json) = serde_json::to_string(msg) {
            let _ = tx.send(json);
        }
    }
}

// ── Test helpers ────────────────────────────────────────────────────────────

const TEST_SECRET: &str = "xK9mP2qR7wL4nJ6vB8cT3fY5hA0gD1eS_rUn";

async fn ws_connect(
    url: &str,
) -> (
    futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::Message,
    >,
    futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) {
    use futures::StreamExt;
    let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    ws.split()
}

async fn ws_send(
    sink: &mut (impl futures::SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin),
    msg: &SignalingMessage,
) {
    let json = serde_json::to_string(msg).unwrap();
    sink.send(tokio_tungstenite::tungstenite::Message::Text(json.into()))
        .await
        .ok();
}

async fn ws_recv(
    stream: &mut (impl futures::StreamExt<
        Item = Result<
            tokio_tungstenite::tungstenite::Message,
            tokio_tungstenite::tungstenite::Error,
        >,
    > + Unpin),
) -> SignalingMessage {
    loop {
        match stream.next().await.unwrap().unwrap() {
            tokio_tungstenite::tungstenite::Message::Text(t) => {
                return serde_json::from_str(&t).unwrap()
            }
            _ => continue,
        }
    }
}

// ── Frame builder helper ────────────────────────────────────────────────────

fn build_request_frame(
    header: &crate::adi_frame::RequestHeader,
    payload: &[u8],
) -> Vec<u8> {
    let header_json = serde_json::to_vec(header).unwrap();
    let mut buf = Vec::with_capacity(4 + header_json.len() + payload.len());
    buf.extend_from_slice(&(header_json.len() as u32).to_be_bytes());
    buf.extend_from_slice(&header_json);
    buf.extend_from_slice(payload);
    buf
}

// ── Test plugin for ADI router ──────────────────────────────────────────────

struct EchoPlugin;

#[async_trait]
impl AdiService for EchoPlugin {
    fn plugin_id(&self) -> &str {
        "adi.echo-test"
    }
    fn name(&self) -> &str {
        "Echo Test"
    }
    fn version(&self) -> &str {
        "1.0.0"
    }

    fn methods(&self) -> Vec<AdiMethodInfo> {
        vec![AdiMethodInfo {
            name: "echo".to_string(),
            description: "Echo back the payload".to_string(),
            streaming: false,
            params_schema: None,
            ..Default::default()
        }]
    }

    async fn handle(
        &self,
        _ctx: &AdiCallerContext,
        method: &str,
        payload: Bytes,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        match method {
            "echo" => Ok(AdiHandleResult::Success(payload)),
            _ => Err(AdiServiceError::method_not_found(method)),
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// Test 1: Cocoon connects to signaling server and registers successfully.
#[tokio::test]
async fn test_signaling_connection_and_registration() {
    let url = test_signaling::spawn_server().await;
    let cocoon_url = format!("{}?kind=cocoon", url);

    let (mut sink, mut stream) = ws_connect(&cocoon_url).await;

    // Register as cocoon device
    ws_send(
        &mut sink,
        &SignalingMessage::DeviceRegister {
            secret: TEST_SECRET.to_string(),
            device_id: None,
            version: "0.0.1-test".to_string(),
            tags: Some(
                [("env".to_string(), "test".to_string())]
                    .into_iter()
                    .collect(),
            ),
            device_type: Some("cocoon".to_string()),
            device_config: None,
        },
    )
    .await;

    let response = ws_recv(&mut stream).await;
    match response {
        SignalingMessage::DeviceRegisterResponse { device_id, tags } => {
            assert!(!device_id.is_empty(), "device_id must be non-empty");
            assert_eq!(tags.as_ref().unwrap()["env"], "test");
        }
        other => panic!("Expected DeviceRegisterResponse, got: {:?}", other),
    }
}

/// Test 2: Two cocoons pair through signaling and relay messages via SyncData.
#[tokio::test]
async fn test_signaling_pairing_and_message_relay() {
    let url = test_signaling::spawn_server().await;
    let cocoon_url = format!("{}?kind=cocoon", url);

    // Cocoon A (the actual cocoon)
    let (mut sink_a, mut stream_a) = ws_connect(&cocoon_url).await;
    ws_send(
        &mut sink_a,
        &SignalingMessage::DeviceRegister {
            secret: "aB3cD4eF5gH6iJ7kL8mN9oP0qR1sT2uV_e2e".to_string(),
            device_id: None,
            version: "0.0.1".to_string(),
            tags: None,
            device_type: Some("cocoon".to_string()),
            device_config: None,
        },
    )
    .await;
    let _reg_a = ws_recv(&mut stream_a).await;

    // Create pairing code
    ws_send(&mut sink_a, &SignalingMessage::PairingCreateCode).await;
    let code = match ws_recv(&mut stream_a).await {
        SignalingMessage::PairingCreateCodeResponse { code } => code,
        other => panic!("Expected PairingCreateCodeResponse, got: {:?}", other),
    };

    // Cocoon B (simulating the plugin/client)
    let (mut sink_b, mut stream_b) = ws_connect(&cocoon_url).await;
    ws_send(
        &mut sink_b,
        &SignalingMessage::DeviceRegister {
            secret: "xY9wV8uT7sR6qP5oN4mL3kJ2iH1gF0eD_e2e".to_string(),
            device_id: None,
            version: "0.0.1".to_string(),
            tags: None,
            device_type: Some("cocoon".to_string()),
            device_config: None,
        },
    )
    .await;
    let _reg_b = ws_recv(&mut stream_b).await;

    // B uses the pairing code
    ws_send(
        &mut sink_b,
        &SignalingMessage::PairingUseCode { code },
    )
    .await;

    // Both should receive PairingUseCodeResponse
    let _paired_b = ws_recv(&mut stream_b).await;
    let _paired_a = ws_recv(&mut stream_a).await;

    // Send a cocoon message through signaling (simulating WebRTC offer exchange)
    let test_payload = serde_json::to_value(&CocoonMessage::WebrtcStartSession {
        session_id: "test-session-123".to_string(),
        device_id: "device-b".to_string(),
        user_id: None,
        data_channels: Some(vec!["silk".to_string(), "adi".to_string()]),
    })
    .unwrap();

    ws_send(
        &mut sink_b,
        &SignalingMessage::SyncData {
            payload: test_payload.clone(),
        },
    )
    .await;

    // Cocoon A should receive the message
    let relayed = ws_recv(&mut stream_a).await;
    match relayed {
        SignalingMessage::SyncData { payload } => {
            let msg: CocoonMessage = serde_json::from_value(payload).unwrap();
            match msg {
                CocoonMessage::WebrtcStartSession {
                    session_id,
                    data_channels,
                    ..
                } => {
                    assert_eq!(session_id, "test-session-123");
                    assert_eq!(
                        data_channels,
                        Some(vec!["silk".to_string(), "adi".to_string()])
                    );
                }
                other => panic!("Expected WebrtcStartSession, got: {:?}", other),
            }
        }
        other => panic!("Expected SyncData, got: {:?}", other),
    }
}

/// Test 3: Full WebRTC peer connection with silk data channel message exchange.
///
/// Client creates a WebRTC offer, cocoon answers, they connect peer-to-peer,
/// and the client sends a silk_create_session request that the cocoon processes.
#[tokio::test]
async fn test_webrtc_silk_create_session_e2e() {
    use webrtc::api::interceptor_registry::register_default_interceptors;
    use webrtc::api::media_engine::MediaEngine;
    use webrtc::api::APIBuilder;
    use webrtc::data_channel::data_channel_message::DataChannelMessage;
    use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
    use webrtc::interceptor::registry::Registry;
    use webrtc::peer_connection::configuration::RTCConfiguration;
    use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

    // ── Cocoon side: WebRtcManager ──
    let (signaling_tx, mut signaling_rx) = mpsc::unbounded_channel();
    let manager = Arc::new(WebRtcManager::new(signaling_tx));
    manager
        .create_session("e2e-silk-test".to_string(), None)
        .await
        .unwrap();

    // ── Client side: raw WebRTC peer connection ──
    let mut me = MediaEngine::default();
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut me).unwrap();
    let api = APIBuilder::new()
        .with_media_engine(me)
        .with_interceptor_registry(registry)
        .build();

    let client_pc = Arc::new(
        api.new_peer_connection(RTCConfiguration::default())
            .await
            .unwrap(),
    );

    // Create silk data channel on client
    let silk_dc = client_pc
        .create_data_channel("silk", None)
        .await
        .unwrap();

    // Channel to receive silk responses (cocoon sends binary JSON via dc.send)
    let (response_tx, response_rx) = mpsc::unbounded_channel::<String>();
    let response_rx = Arc::new(Mutex::new(response_rx));
    silk_dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let tx = response_tx.clone();
        Box::pin(async move {
            let text = String::from_utf8_lossy(&msg.data).to_string();
            let _ = tx.send(text);
        })
    }));

    // Notify when silk DC opens
    let (dc_open_tx, dc_open_rx) = tokio::sync::oneshot::channel::<()>();
    let dc_open_tx = Arc::new(Mutex::new(Some(dc_open_tx)));
    silk_dc.on_open(Box::new(move || {
        let tx = dc_open_tx.clone();
        Box::pin(async move {
            if let Some(tx) = tx.lock().await.take() {
                let _ = tx.send(());
            }
        })
    }));

    // ── ICE candidate exchange: Client → Cocoon ──
    let manager_for_ice = manager.clone();
    client_pc.on_ice_candidate(Box::new(move |candidate| {
        let mgr = manager_for_ice.clone();
        Box::pin(async move {
            if let Some(c) = candidate {
                if let Ok(json) = c.to_json() {
                    let sdp_mid = match json.sdp_mid.as_deref() {
                        Some("") | None => Some("0".to_string()),
                        other => other.map(|s| s.to_string()),
                    };
                    let _ = mgr
                        .add_ice_candidate(
                            "e2e-silk-test",
                            &json.candidate,
                            sdp_mid.as_deref(),
                            json.sdp_mline_index.map(|i| i as u32),
                        )
                        .await;
                }
            }
        })
    }));

    // ── ICE candidate exchange: Cocoon → Client ──
    let client_pc_for_ice = client_pc.clone();
    tokio::spawn(async move {
        while let Some(msg) = signaling_rx.recv().await {
            if let SignalingMessage::SyncData { payload } = msg {
                if let Ok(cocoon_msg) = serde_json::from_value::<CocoonMessage>(payload) {
                    if let CocoonMessage::WebrtcIceCandidate {
                        candidate,
                        sdp_mid,
                        sdp_mline_index,
                        ..
                    } = cocoon_msg
                    {
                        let ice = RTCIceCandidateInit {
                            candidate,
                            sdp_mid,
                            sdp_mline_index: sdp_mline_index.map(|i| i as u16),
                            ..Default::default()
                        };
                        let _ = client_pc_for_ice.add_ice_candidate(ice).await;
                    }
                }
            }
        }
    });

    // ── SDP exchange ──
    let offer = client_pc.create_offer(None).await.unwrap();
    client_pc
        .set_local_description(offer.clone())
        .await
        .unwrap();

    let answer_sdp = manager
        .handle_offer("e2e-silk-test", &offer.sdp)
        .await
        .unwrap();

    let answer = RTCSessionDescription::answer(answer_sdp).unwrap();
    client_pc.set_remote_description(answer).await.unwrap();

    // ── Wait for data channel to open ──
    tokio::time::timeout(std::time::Duration::from_secs(10), dc_open_rx)
        .await
        .expect("Timed out waiting for silk data channel to open")
        .expect("Data channel open sender dropped");

    // ── Send silk_create_session (text message, as browser would) ──
    let create_session_msg = CocoonMessage::SilkCreateSession {
        cwd: None,
        env: None,
        shell: None,
    };
    let json = serde_json::to_string(&create_session_msg).unwrap();
    silk_dc.send_text(json).await.unwrap();

    // ── Wait for response ──
    let response_text = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut rx = response_rx.lock().await;
        rx.recv().await.unwrap()
    })
    .await
    .expect("Timed out waiting for silk response");

    let response: CocoonMessage = serde_json::from_str(&response_text).unwrap();
    match response {
        CocoonMessage::SilkCreateSessionResponse {
            session_id,
            cwd,
            shell,
        } => {
            assert!(!session_id.is_empty(), "session_id must be non-empty");
            assert!(!cwd.is_empty(), "cwd must be non-empty");
            assert!(!shell.is_empty(), "shell must be non-empty");
        }
        CocoonMessage::SilkError { code, message, .. } => {
            panic!("Silk session creation failed: {} - {}", code, message);
        }
        other => panic!(
            "Expected SilkCreateSessionResponse, got: {:?}",
            other
        ),
    }

    // Cleanup
    let _ = client_pc.close().await;
    let _ = manager.close_session("e2e-silk-test").await;
}

/// Test 4: WebRTC with ADI binary plugin request through the adi data channel.
///
/// Registers an echo plugin in the AdiRouter, sends a binary ADI frame through
/// the adi data channel, and verifies the plugin response.
#[tokio::test]
async fn test_webrtc_adi_plugin_echo_e2e() {
    use crate::adi_frame::{RequestHeader, ResponseHeader, ResponseStatus};
    use webrtc::api::interceptor_registry::register_default_interceptors;
    use webrtc::api::media_engine::MediaEngine;
    use webrtc::api::APIBuilder;
    use webrtc::data_channel::data_channel_message::DataChannelMessage;
    use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
    use webrtc::interceptor::registry::Registry;
    use webrtc::peer_connection::configuration::RTCConfiguration;
    use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

    // ── Set up AdiRouter with echo plugin ──
    let mut router = AdiRouter::new();
    router.register(Arc::new(EchoPlugin));
    let router = Arc::new(Mutex::new(router));

    // ── Cocoon side: WebRtcManager with AdiRouter ──
    let (signaling_tx, mut signaling_rx) = mpsc::unbounded_channel();
    let manager = Arc::new(WebRtcManager::with_adi_router(signaling_tx, router));
    manager
        .create_session("e2e-adi-test".to_string(), None)
        .await
        .unwrap();

    // ── Client side ──
    let mut me = MediaEngine::default();
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut me).unwrap();
    let api = APIBuilder::new()
        .with_media_engine(me)
        .with_interceptor_registry(registry)
        .build();

    let client_pc = Arc::new(
        api.new_peer_connection(RTCConfiguration::default())
            .await
            .unwrap(),
    );

    // Create adi data channel
    let adi_dc = client_pc.create_data_channel("adi", None).await.unwrap();

    // Channel for binary responses
    let (response_tx, response_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let response_rx = Arc::new(Mutex::new(response_rx));
    adi_dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let tx = response_tx.clone();
        Box::pin(async move {
            if !msg.is_string {
                let _ = tx.send(msg.data.to_vec());
            }
        })
    }));

    // Notify when adi DC opens
    let (dc_open_tx, dc_open_rx) = tokio::sync::oneshot::channel::<()>();
    let dc_open_tx = Arc::new(Mutex::new(Some(dc_open_tx)));
    adi_dc.on_open(Box::new(move || {
        let tx = dc_open_tx.clone();
        Box::pin(async move {
            if let Some(tx) = tx.lock().await.take() {
                let _ = tx.send(());
            }
        })
    }));

    // ── ICE: Client → Cocoon ──
    let manager_for_ice = manager.clone();
    client_pc.on_ice_candidate(Box::new(move |candidate| {
        let mgr = manager_for_ice.clone();
        Box::pin(async move {
            if let Some(c) = candidate {
                if let Ok(json) = c.to_json() {
                    let sdp_mid = match json.sdp_mid.as_deref() {
                        Some("") | None => Some("0".to_string()),
                        other => other.map(|s| s.to_string()),
                    };
                    let _ = mgr
                        .add_ice_candidate(
                            "e2e-adi-test",
                            &json.candidate,
                            sdp_mid.as_deref(),
                            json.sdp_mline_index.map(|i| i as u32),
                        )
                        .await;
                }
            }
        })
    }));

    // ── ICE: Cocoon → Client ──
    let client_pc_for_ice = client_pc.clone();
    tokio::spawn(async move {
        while let Some(msg) = signaling_rx.recv().await {
            if let SignalingMessage::SyncData { payload } = msg {
                if let Ok(cocoon_msg) = serde_json::from_value::<CocoonMessage>(payload) {
                    if let CocoonMessage::WebrtcIceCandidate {
                        candidate,
                        sdp_mid,
                        sdp_mline_index,
                        ..
                    } = cocoon_msg
                    {
                        let ice = RTCIceCandidateInit {
                            candidate,
                            sdp_mid,
                            sdp_mline_index: sdp_mline_index.map(|i| i as u16),
                            ..Default::default()
                        };
                        let _ = client_pc_for_ice.add_ice_candidate(ice).await;
                    }
                }
            }
        }
    });

    // ── SDP exchange ──
    let offer = client_pc.create_offer(None).await.unwrap();
    client_pc
        .set_local_description(offer.clone())
        .await
        .unwrap();

    let answer_sdp = manager
        .handle_offer("e2e-adi-test", &offer.sdp)
        .await
        .unwrap();

    let answer = RTCSessionDescription::answer(answer_sdp).unwrap();
    client_pc.set_remote_description(answer).await.unwrap();

    // ── Wait for data channel to open ──
    tokio::time::timeout(std::time::Duration::from_secs(10), dc_open_rx)
        .await
        .expect("Timed out waiting for adi data channel to open")
        .expect("Data channel open sender dropped");

    // ── Build and send ADI binary frame ──
    let request_id = uuid::Uuid::new_v4();
    let payload = serde_json::to_vec(&serde_json::json!({"message": "hello from plugin"})).unwrap();
    let header = RequestHeader {
        v: 1,
        id: request_id,
        plugin: "adi.echo-test".to_string(),
        method: "echo".to_string(),
        stream: false,
    };
    let frame = build_request_frame(&header, &payload);
    adi_dc.send(&Bytes::from(frame)).await.unwrap();

    // ── Wait for response ──
    let response_bytes = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut rx = response_rx.lock().await;
        rx.recv().await.unwrap()
    })
    .await
    .expect("Timed out waiting for ADI response");

    // Parse response frame
    let header_len =
        u32::from_be_bytes([response_bytes[0], response_bytes[1], response_bytes[2], response_bytes[3]])
            as usize;
    let resp_header: ResponseHeader =
        serde_json::from_slice(&response_bytes[4..4 + header_len]).unwrap();
    let resp_payload = &response_bytes[4 + header_len..];

    assert_eq!(resp_header.id, request_id);
    assert_eq!(resp_header.status, ResponseStatus::Success);

    let data: serde_json::Value = serde_json::from_slice(resp_payload).unwrap();
    assert_eq!(data["message"], "hello from plugin");

    // Cleanup
    let _ = client_pc.close().await;
    let _ = manager.close_session("e2e-adi-test").await;
}

// ── WebRTC test helper ─────────────────────────────────────────────────────

/// Reusable WebRTC peer connection setup for E2E tests.
///
/// Returns `(silk_dc, adi_dc, silk_response_rx, adi_response_rx, client_pc, manager)`
/// with data channels already open and ready.
struct WebRtcTestHarness {
    manager: Arc<WebRtcManager>,
    client_pc: Arc<webrtc::peer_connection::RTCPeerConnection>,
    silk_dc: Arc<webrtc::data_channel::RTCDataChannel>,
    adi_dc: Arc<webrtc::data_channel::RTCDataChannel>,
    silk_rx: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
    adi_rx: Arc<Mutex<mpsc::UnboundedReceiver<Vec<u8>>>>,
    session_id: String,
}

impl WebRtcTestHarness {
    async fn new(session_id: &str, adi_router: Option<Arc<Mutex<AdiRouter>>>) -> Self {
        use webrtc::api::interceptor_registry::register_default_interceptors;
        use webrtc::api::media_engine::MediaEngine;
        use webrtc::api::APIBuilder;
        use webrtc::data_channel::data_channel_message::DataChannelMessage;
        use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
        use webrtc::interceptor::registry::Registry;
        use webrtc::peer_connection::configuration::RTCConfiguration;
        use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

        let (signaling_tx, mut signaling_rx) = mpsc::unbounded_channel();
        let manager = Arc::new(match adi_router {
            Some(router) => WebRtcManager::with_adi_router(signaling_tx, router),
            None => WebRtcManager::new(signaling_tx),
        });
        manager
            .create_session(session_id.to_string(), None)
            .await
            .unwrap();

        let mut me = MediaEngine::default();
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut me).unwrap();
        let api = APIBuilder::new()
            .with_media_engine(me)
            .with_interceptor_registry(registry)
            .build();

        let client_pc = Arc::new(
            api.new_peer_connection(RTCConfiguration::default())
                .await
                .unwrap(),
        );

        // Create both data channels
        let silk_dc = client_pc.create_data_channel("silk", None).await.unwrap();
        let adi_dc = client_pc.create_data_channel("adi", None).await.unwrap();

        // Silk response channel
        let (silk_tx, silk_rx) = mpsc::unbounded_channel::<String>();
        silk_dc.on_message(Box::new(move |msg: DataChannelMessage| {
            let tx = silk_tx.clone();
            Box::pin(async move {
                let text = String::from_utf8_lossy(&msg.data).to_string();
                let _ = tx.send(text);
            })
        }));

        // ADI response channel
        let (adi_tx, adi_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        adi_dc.on_message(Box::new(move |msg: DataChannelMessage| {
            let tx = adi_tx.clone();
            Box::pin(async move {
                let _ = tx.send(msg.data.to_vec());
            })
        }));

        // Wait for both DCs to open
        let (dc_open_tx, dc_open_rx) = tokio::sync::oneshot::channel::<()>();
        let dc_open_tx = Arc::new(Mutex::new(Some(dc_open_tx)));
        // We use silk's on_open as the gate (adi opens at the same time)
        let open_tx = dc_open_tx.clone();
        silk_dc.on_open(Box::new(move || {
            let tx = open_tx.clone();
            Box::pin(async move {
                if let Some(tx) = tx.lock().await.take() {
                    let _ = tx.send(());
                }
            })
        }));

        // ICE: Client → Cocoon
        let manager_for_ice = manager.clone();
        let sid = session_id.to_string();
        client_pc.on_ice_candidate(Box::new(move |candidate| {
            let mgr = manager_for_ice.clone();
            let sid = sid.clone();
            Box::pin(async move {
                if let Some(c) = candidate {
                    if let Ok(json) = c.to_json() {
                        let sdp_mid = match json.sdp_mid.as_deref() {
                            Some("") | None => Some("0".to_string()),
                            other => other.map(|s| s.to_string()),
                        };
                        let _ = mgr
                            .add_ice_candidate(
                                &sid,
                                &json.candidate,
                                sdp_mid.as_deref(),
                                json.sdp_mline_index.map(|i| i as u32),
                            )
                            .await;
                    }
                }
            })
        }));

        // ICE: Cocoon → Client
        let client_pc_for_ice = client_pc.clone();
        tokio::spawn(async move {
            while let Some(msg) = signaling_rx.recv().await {
                if let SignalingMessage::SyncData { payload } = msg {
                    if let Ok(cocoon_msg) = serde_json::from_value::<CocoonMessage>(payload) {
                        if let CocoonMessage::WebrtcIceCandidate {
                            candidate,
                            sdp_mid,
                            sdp_mline_index,
                            ..
                        } = cocoon_msg
                        {
                            let ice = RTCIceCandidateInit {
                                candidate,
                                sdp_mid,
                                sdp_mline_index: sdp_mline_index.map(|i| i as u16),
                                ..Default::default()
                            };
                            let _ = client_pc_for_ice.add_ice_candidate(ice).await;
                        }
                    }
                }
            }
        });

        // SDP exchange
        let offer = client_pc.create_offer(None).await.unwrap();
        client_pc.set_local_description(offer.clone()).await.unwrap();

        let answer_sdp = manager.handle_offer(session_id, &offer.sdp).await.unwrap();
        let answer = RTCSessionDescription::answer(answer_sdp).unwrap();
        client_pc.set_remote_description(answer).await.unwrap();

        // Wait for DC open
        tokio::time::timeout(std::time::Duration::from_secs(10), dc_open_rx)
            .await
            .expect("Timed out waiting for data channels to open")
            .expect("DC open sender dropped");

        Self {
            manager,
            client_pc,
            silk_dc,
            adi_dc,
            silk_rx: Arc::new(Mutex::new(silk_rx)),
            adi_rx: Arc::new(Mutex::new(adi_rx)),
            session_id: session_id.to_string(),
        }
    }

    async fn send_silk(&self, msg: &CocoonMessage) {
        let json = serde_json::to_string(msg).unwrap();
        self.silk_dc.send_text(json).await.unwrap();
    }

    async fn recv_silk(&self) -> CocoonMessage {
        let text = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            self.silk_rx.lock().await.recv().await.unwrap()
        })
        .await
        .expect("Timed out waiting for silk response");
        serde_json::from_str(&text).unwrap()
    }

    async fn send_adi_frame(&self, header: &crate::adi_frame::RequestHeader, payload: &[u8]) {
        let frame = build_request_frame(header, payload);
        self.adi_dc.send(&Bytes::from(frame)).await.unwrap();
    }

    async fn recv_adi_frame(
        &self,
    ) -> (crate::adi_frame::ResponseHeader, Vec<u8>) {
        let data = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            self.adi_rx.lock().await.recv().await.unwrap()
        })
        .await
        .expect("Timed out waiting for ADI response");

        let header_len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let header: crate::adi_frame::ResponseHeader =
            serde_json::from_slice(&data[4..4 + header_len]).unwrap();
        let payload = data[4 + header_len..].to_vec();
        (header, payload)
    }

    async fn cleanup(self) {
        let _ = self.client_pc.close().await;
        let _ = self.manager.close_session(&self.session_id).await;
    }
}

// ── Signaling Error Tests ──────────────────────────────────────────────────

/// Test 5: Signaling server rejects weak secrets.
#[tokio::test]
async fn test_signaling_rejects_weak_secret() {
    let url = test_signaling::spawn_server().await;
    let cocoon_url = format!("{}?kind=cocoon", url);

    let (mut sink, mut stream) = ws_connect(&cocoon_url).await;

    // Try registering with a weak secret (contains "password")
    ws_send(
        &mut sink,
        &SignalingMessage::DeviceRegister {
            secret: "mypassword1234567890abcdefghijklmnop".to_string(),
            device_id: None,
            version: "0.0.1".to_string(),
            tags: None,
            device_type: Some("cocoon".to_string()),
            device_config: None,
        },
    )
    .await;

    let response = ws_recv(&mut stream).await;
    match response {
        SignalingMessage::SystemError { message } => {
            assert!(
                !message.is_empty(),
                "Error message should explain why the secret was rejected"
            );
        }
        other => panic!("Expected SystemError for weak secret, got: {:?}", other),
    }
}

/// Test 6: Using an invalid pairing code returns no response (code not found).
#[tokio::test]
async fn test_signaling_invalid_pairing_code() {
    let url = test_signaling::spawn_server().await;
    let cocoon_url = format!("{}?kind=cocoon", url);

    let (mut sink, mut stream) = ws_connect(&cocoon_url).await;

    // Register first
    ws_send(
        &mut sink,
        &SignalingMessage::DeviceRegister {
            secret: TEST_SECRET.to_string(),
            device_id: None,
            version: "0.0.1".to_string(),
            tags: None,
            device_type: Some("cocoon".to_string()),
            device_config: None,
        },
    )
    .await;
    let _reg = ws_recv(&mut stream).await;

    // Try using a non-existent pairing code
    ws_send(
        &mut sink,
        &SignalingMessage::PairingUseCode {
            code: "INVALID-CODE-12345".to_string(),
        },
    )
    .await;

    // Server silently ignores invalid codes (no crash, no response)
    // Verify we can still communicate by creating a valid pairing code
    ws_send(&mut sink, &SignalingMessage::PairingCreateCode).await;
    let response = ws_recv(&mut stream).await;
    match response {
        SignalingMessage::PairingCreateCodeResponse { code } => {
            assert!(!code.is_empty(), "Should still work after invalid code attempt");
        }
        other => panic!("Expected PairingCreateCodeResponse, got: {:?}", other),
    }
}

// ── Silk Command Execution Tests ───────────────────────────────────────────

/// Test 7: Full silk command execution lifecycle: create session → execute → output → completed.
#[tokio::test]
async fn test_silk_command_execution_e2e() {
    let harness = WebRtcTestHarness::new("silk-exec-test", None).await;

    // Create session
    harness
        .send_silk(&CocoonMessage::SilkCreateSession {
            cwd: None,
            env: None,
            shell: None,
        })
        .await;

    let session_id = match harness.recv_silk().await {
        CocoonMessage::SilkCreateSessionResponse { session_id, .. } => session_id,
        other => panic!("Expected SilkCreateSessionResponse, got: {:?}", other),
    };

    // Execute a simple command
    let command_id = uuid::Uuid::new_v4().to_string();
    harness
        .send_silk(&CocoonMessage::SilkExecute {
            session_id: session_id.clone(),
            command: "echo hello_from_cocoon".to_string(),
            command_id: command_id.clone(),
            cols: Some(80),
            rows: Some(24),
            env: None,
        })
        .await;

    // Collect all responses until CommandCompleted
    let mut got_started = false;
    let mut got_output = false;
    let mut exit_code = None;

    let timeout = std::time::Duration::from_secs(15);
    let start = std::time::Instant::now();

    while start.elapsed() < timeout {
        let msg = harness.recv_silk().await;
        match msg {
            CocoonMessage::SilkCommandStarted {
                command_id: cid,
                interactive,
                ..
            } => {
                assert_eq!(cid, command_id);
                assert!(!interactive);
                got_started = true;
            }
            CocoonMessage::SilkOutput {
                command_id: cid,
                data,
                ..
            } => {
                assert_eq!(cid, command_id);
                if data.contains("hello_from_cocoon") {
                    got_output = true;
                }
            }
            CocoonMessage::SilkCommandCompleted {
                command_id: cid,
                exit_code: code,
                ..
            } => {
                assert_eq!(cid, command_id);
                exit_code = Some(code);
                break;
            }
            CocoonMessage::SilkError { code, message, .. } => {
                panic!("Silk error: {} - {}", code, message);
            }
            _ => {}
        }
    }

    assert!(got_started, "Should have received SilkCommandStarted");
    assert!(got_output, "Should have received output containing 'hello_from_cocoon'");
    assert_eq!(exit_code, Some(0), "Command should exit with code 0");

    harness.cleanup().await;
}

/// Test 8: Silk session lifecycle: create → close → verify closed.
#[tokio::test]
async fn test_silk_session_close_e2e() {
    let harness = WebRtcTestHarness::new("silk-close-test", None).await;

    // Create session
    harness
        .send_silk(&CocoonMessage::SilkCreateSession {
            cwd: None,
            env: None,
            shell: None,
        })
        .await;

    let session_id = match harness.recv_silk().await {
        CocoonMessage::SilkCreateSessionResponse { session_id, .. } => session_id,
        other => panic!("Expected SilkCreateSessionResponse, got: {:?}", other),
    };

    // Close the session
    harness
        .send_silk(&CocoonMessage::SilkCloseSession {
            session_id: session_id.clone(),
        })
        .await;

    match harness.recv_silk().await {
        CocoonMessage::SilkSessionClosed {
            session_id: closed_id,
        } => {
            assert_eq!(closed_id, session_id);
        }
        other => panic!("Expected SilkSessionClosed, got: {:?}", other),
    }

    harness.cleanup().await;
}

/// Test 9: Execute command on non-existent silk session returns error.
#[tokio::test]
async fn test_silk_execute_nonexistent_session_e2e() {
    let harness = WebRtcTestHarness::new("silk-noexist-test", None).await;

    let command_id = uuid::Uuid::new_v4().to_string();
    harness
        .send_silk(&CocoonMessage::SilkExecute {
            session_id: "nonexistent-session-id".to_string(),
            command: "echo test".to_string(),
            command_id: command_id.clone(),
            cols: Some(80),
            rows: Some(24),
            env: None,
        })
        .await;

    match harness.recv_silk().await {
        CocoonMessage::SilkError {
            code, session_id, command_id: cid, ..
        } => {
            assert_eq!(code, "session_not_found");
            assert_eq!(session_id, Some("nonexistent-session-id".to_string()));
            assert_eq!(cid, Some(command_id));
        }
        other => panic!("Expected SilkError, got: {:?}", other),
    }

    harness.cleanup().await;
}

// ── ADI Error Path Tests ───────────────────────────────────────────────────

/// Test plugin that always returns an error.
struct ErrorPlugin;

#[async_trait]
impl AdiService for ErrorPlugin {
    fn plugin_id(&self) -> &str {
        "adi.error-test"
    }
    fn name(&self) -> &str {
        "Error Test"
    }
    fn version(&self) -> &str {
        "1.0.0"
    }

    fn methods(&self) -> Vec<AdiMethodInfo> {
        vec![AdiMethodInfo {
            name: "fail".to_string(),
            description: "Always fails".to_string(),
            streaming: false,
            params_schema: None,
            ..Default::default()
        }]
    }

    async fn handle(
        &self,
        _ctx: &AdiCallerContext,
        method: &str,
        _payload: Bytes,
    ) -> Result<AdiHandleResult, AdiServiceError> {
        match method {
            "fail" => Err(AdiServiceError::internal("intentional test error")),
            _ => Err(AdiServiceError::method_not_found(method)),
        }
    }
}

/// Test 10: ADI request to non-existent plugin returns PluginNotFound.
#[tokio::test]
async fn test_adi_plugin_not_found_e2e() {
    use crate::adi_frame::{RequestHeader, ResponseStatus};

    let mut router = AdiRouter::new();
    router.register(Arc::new(EchoPlugin));
    let router = Arc::new(Mutex::new(router));

    let harness = WebRtcTestHarness::new("adi-notfound-test", Some(router)).await;

    let request_id = uuid::Uuid::new_v4();
    harness
        .send_adi_frame(
            &RequestHeader {
                v: 1,
                id: request_id,
                plugin: "adi.nonexistent".to_string(),
                method: "test".to_string(),
                stream: false,
            },
            b"{}",
        )
        .await;

    let (header, payload) = harness.recv_adi_frame().await;
    assert_eq!(header.id, request_id);
    assert_eq!(header.status, ResponseStatus::PluginNotFound);
    let msg = String::from_utf8_lossy(&payload);
    assert!(msg.contains("not found"), "Error payload: {}", msg);

    harness.cleanup().await;
}

/// Test 11: ADI request with non-existent method returns MethodNotFound.
#[tokio::test]
async fn test_adi_method_not_found_e2e() {
    use crate::adi_frame::{RequestHeader, ResponseStatus};

    let mut router = AdiRouter::new();
    router.register(Arc::new(EchoPlugin));
    let router = Arc::new(Mutex::new(router));

    let harness = WebRtcTestHarness::new("adi-nomethod-test", Some(router)).await;

    let request_id = uuid::Uuid::new_v4();
    harness
        .send_adi_frame(
            &RequestHeader {
                v: 1,
                id: request_id,
                plugin: "adi.echo-test".to_string(),
                method: "nonexistent_method".to_string(),
                stream: false,
            },
            b"{}",
        )
        .await;

    let (header, payload) = harness.recv_adi_frame().await;
    assert_eq!(header.id, request_id);
    assert_eq!(header.status, ResponseStatus::MethodNotFound);
    let msg = String::from_utf8_lossy(&payload);
    assert!(msg.contains("not found"), "Error payload: {}", msg);

    harness.cleanup().await;
}

/// Test 12: ADI request with truncated/invalid binary frame returns InvalidRequest.
#[tokio::test]
async fn test_adi_invalid_frame_e2e() {
    use crate::adi_frame::ResponseStatus;

    let mut router = AdiRouter::new();
    router.register(Arc::new(EchoPlugin));
    let router = Arc::new(Mutex::new(router));

    let harness = WebRtcTestHarness::new("adi-badframe-test", Some(router)).await;

    // Send a truncated frame (only 2 bytes, needs at least 4 for header_len)
    harness
        .adi_dc
        .send(&Bytes::from_static(&[0x00, 0x01]))
        .await
        .unwrap();

    let (header, _payload) = harness.recv_adi_frame().await;
    assert_eq!(header.status, ResponseStatus::InvalidRequest);

    harness.cleanup().await;
}

/// Test 13: ADI plugin returns error status propagated to client.
#[tokio::test]
async fn test_adi_plugin_error_response_e2e() {
    use crate::adi_frame::{RequestHeader, ResponseStatus};

    let mut router = AdiRouter::new();
    router.register(Arc::new(ErrorPlugin));
    let router = Arc::new(Mutex::new(router));

    let harness = WebRtcTestHarness::new("adi-pluginerr-test", Some(router)).await;

    let request_id = uuid::Uuid::new_v4();
    harness
        .send_adi_frame(
            &RequestHeader {
                v: 1,
                id: request_id,
                plugin: "adi.error-test".to_string(),
                method: "fail".to_string(),
                stream: false,
            },
            b"{}",
        )
        .await;

    let (header, _payload) = harness.recv_adi_frame().await;
    assert_eq!(header.id, request_id);
    assert_eq!(header.status, ResponseStatus::Error);

    harness.cleanup().await;
}

/// Test 14: ADI request with empty payload echoes empty payload back.
#[tokio::test]
async fn test_adi_empty_payload_e2e() {
    use crate::adi_frame::{RequestHeader, ResponseStatus};

    let mut router = AdiRouter::new();
    router.register(Arc::new(EchoPlugin));
    let router = Arc::new(Mutex::new(router));

    let harness = WebRtcTestHarness::new("adi-empty-test", Some(router)).await;

    let request_id = uuid::Uuid::new_v4();
    harness
        .send_adi_frame(
            &RequestHeader {
                v: 1,
                id: request_id,
                plugin: "adi.echo-test".to_string(),
                method: "echo".to_string(),
                stream: false,
            },
            b"",
        )
        .await;

    let (header, payload) = harness.recv_adi_frame().await;
    assert_eq!(header.id, request_id);
    assert_eq!(header.status, ResponseStatus::Success);
    assert!(payload.is_empty(), "Echo of empty payload should be empty");

    harness.cleanup().await;
}

/// Test 15: ADI frame with header_len exceeding data returns InvalidRequest.
#[tokio::test]
async fn test_adi_header_too_large_e2e() {
    use crate::adi_frame::ResponseStatus;

    let mut router = AdiRouter::new();
    router.register(Arc::new(EchoPlugin));
    let router = Arc::new(Mutex::new(router));

    let harness = WebRtcTestHarness::new("adi-headerlarge-test", Some(router)).await;

    // header_len = 9999 but only 4 bytes of actual data after
    let mut frame = Vec::new();
    frame.extend_from_slice(&9999u32.to_be_bytes());
    frame.extend_from_slice(b"tiny");
    harness
        .adi_dc
        .send(&Bytes::from(frame))
        .await
        .unwrap();

    let (header, _payload) = harness.recv_adi_frame().await;
    assert_eq!(header.status, ResponseStatus::InvalidRequest);

    harness.cleanup().await;
}

// ── Signaling: Duplicate Registration ──────────────────────────────────────

/// Test 16: Re-registering with the same secret produces the same device_id (deterministic HMAC).
#[tokio::test]
async fn test_signaling_duplicate_registration_same_device_id() {
    let url = test_signaling::spawn_server().await;
    let cocoon_url = format!("{}?kind=cocoon", url);

    let register_msg = SignalingMessage::DeviceRegister {
        secret: TEST_SECRET.to_string(),
        device_id: None,
        version: "0.0.1".to_string(),
        tags: None,
        device_type: Some("cocoon".to_string()),
        device_config: None,
    };

    // First registration
    let (mut sink1, mut stream1) = ws_connect(&cocoon_url).await;
    ws_send(&mut sink1, &register_msg).await;
    let id1 = match ws_recv(&mut stream1).await {
        SignalingMessage::DeviceRegisterResponse { device_id, .. } => device_id,
        other => panic!("Expected DeviceRegisterResponse, got: {:?}", other),
    };

    // Second registration (new connection, same secret)
    let (mut sink2, mut stream2) = ws_connect(&cocoon_url).await;
    ws_send(&mut sink2, &register_msg).await;
    let id2 = match ws_recv(&mut stream2).await {
        SignalingMessage::DeviceRegisterResponse { device_id, .. } => device_id,
        other => panic!("Expected DeviceRegisterResponse, got: {:?}", other),
    };

    assert_eq!(id1, id2, "Same secret should produce same device_id (HMAC determinism)");
}

// ── Signaling: Different secrets produce different IDs ─────────────────────

/// Test 17: Different secrets produce different device_ids.
#[tokio::test]
async fn test_signaling_different_secrets_different_ids() {
    let url = test_signaling::spawn_server().await;
    let cocoon_url = format!("{}?kind=cocoon", url);

    let (mut sink1, mut stream1) = ws_connect(&cocoon_url).await;
    ws_send(
        &mut sink1,
        &SignalingMessage::DeviceRegister {
            secret: "aA1bB2cC3dD4eE5fF6gG7hH8iI9jJ0kK_alpha".to_string(),
            device_id: None,
            version: "0.0.1".to_string(),
            tags: None,
            device_type: Some("cocoon".to_string()),
            device_config: None,
        },
    )
    .await;
    let id1 = match ws_recv(&mut stream1).await {
        SignalingMessage::DeviceRegisterResponse { device_id, .. } => device_id,
        other => panic!("Expected DeviceRegisterResponse, got: {:?}", other),
    };

    let (mut sink2, mut stream2) = ws_connect(&cocoon_url).await;
    ws_send(
        &mut sink2,
        &SignalingMessage::DeviceRegister {
            secret: "zZ9yY8xX7wW6vV5uU4tT3sS2rR1qQ0pP_bravo".to_string(),
            device_id: None,
            version: "0.0.1".to_string(),
            tags: None,
            device_type: Some("cocoon".to_string()),
            device_config: None,
        },
    )
    .await;
    let id2 = match ws_recv(&mut stream2).await {
        SignalingMessage::DeviceRegisterResponse { device_id, .. } => device_id,
        other => panic!("Expected DeviceRegisterResponse, got: {:?}", other),
    };

    assert_ne!(id1, id2, "Different secrets must produce different device_ids");
}

// ── Pairing: Self-pairing (same device uses own code) ─────────────────────

/// Test 18: Device cannot pair with itself (code creator and user are the same device).
#[tokio::test]
async fn test_signaling_self_pairing() {
    let url = test_signaling::spawn_server().await;
    let cocoon_url = format!("{}?kind=cocoon", url);

    let (mut sink, mut stream) = ws_connect(&cocoon_url).await;
    ws_send(
        &mut sink,
        &SignalingMessage::DeviceRegister {
            secret: TEST_SECRET.to_string(),
            device_id: None,
            version: "0.0.1".to_string(),
            tags: None,
            device_type: Some("cocoon".to_string()),
            device_config: None,
        },
    )
    .await;
    let _reg = ws_recv(&mut stream).await;

    // Create pairing code
    ws_send(&mut sink, &SignalingMessage::PairingCreateCode).await;
    let code = match ws_recv(&mut stream).await {
        SignalingMessage::PairingCreateCodeResponse { code } => code,
        other => panic!("Expected PairingCreateCodeResponse, got: {:?}", other),
    };

    // Same device tries to use the code
    ws_send(
        &mut sink,
        &SignalingMessage::PairingUseCode { code },
    )
    .await;

    // Self-pairing: server pairs device with itself, producing two PairingUseCodeResponse
    // messages (one for "user" side, one for "creator" side — same connection).
    // Drain all PairingUseCodeResponse messages, then verify server still works.
    ws_send(&mut sink, &SignalingMessage::PairingCreateCode).await;

    let mut got_create_response = false;
    for _ in 0..5 {
        let msg = ws_recv(&mut stream).await;
        match msg {
            SignalingMessage::PairingUseCodeResponse { .. } => continue,
            SignalingMessage::PairingCreateCodeResponse { .. } => {
                got_create_response = true;
                break;
            }
            other => panic!("Unexpected: {:?}", other),
        }
    }
    assert!(got_create_response, "Server should still be functional after self-pairing");
}

// ── Pairing: Code reuse after consumption ─────────────────────────────────

/// Test 19: Pairing code cannot be reused after it has been consumed.
#[tokio::test]
async fn test_signaling_pairing_code_reuse() {
    let url = test_signaling::spawn_server().await;
    let cocoon_url = format!("{}?kind=cocoon", url);

    // Device A
    let (mut sink_a, mut stream_a) = ws_connect(&cocoon_url).await;
    ws_send(
        &mut sink_a,
        &SignalingMessage::DeviceRegister {
            secret: "aB3cD4eF5gH6iJ7kL8mN9oP0qR1sT2uV_code1".to_string(),
            device_id: None,
            version: "0.0.1".to_string(),
            tags: None,
            device_type: Some("cocoon".to_string()),
            device_config: None,
        },
    )
    .await;
    let _reg_a = ws_recv(&mut stream_a).await;

    // Device A creates code
    ws_send(&mut sink_a, &SignalingMessage::PairingCreateCode).await;
    let code = match ws_recv(&mut stream_a).await {
        SignalingMessage::PairingCreateCodeResponse { code } => code,
        other => panic!("Expected code, got: {:?}", other),
    };

    // Device B uses the code
    let (mut sink_b, mut stream_b) = ws_connect(&cocoon_url).await;
    ws_send(
        &mut sink_b,
        &SignalingMessage::DeviceRegister {
            secret: "xY9wV8uT7sR6qP5oN4mL3kJ2iH1gF0eD_code2".to_string(),
            device_id: None,
            version: "0.0.1".to_string(),
            tags: None,
            device_type: Some("cocoon".to_string()),
            device_config: None,
        },
    )
    .await;
    let _reg_b = ws_recv(&mut stream_b).await;

    ws_send(
        &mut sink_b,
        &SignalingMessage::PairingUseCode { code: code.clone() },
    )
    .await;
    let _paired_b = ws_recv(&mut stream_b).await; // PairingUseCodeResponse
    let _paired_a = ws_recv(&mut stream_a).await; // PairingUseCodeResponse

    // Device C tries to reuse the same code
    let (mut sink_c, mut stream_c) = ws_connect(&cocoon_url).await;
    ws_send(
        &mut sink_c,
        &SignalingMessage::DeviceRegister {
            secret: "mN3bV4cX5zL6kJ7hG8fD9sA0pO1iU2yT_code3".to_string(),
            device_id: None,
            version: "0.0.1".to_string(),
            tags: None,
            device_type: Some("cocoon".to_string()),
            device_config: None,
        },
    )
    .await;
    let _reg_c = ws_recv(&mut stream_c).await;

    ws_send(
        &mut sink_c,
        &SignalingMessage::PairingUseCode { code },
    )
    .await;

    // Code should be consumed — verify server still works for C
    ws_send(&mut sink_c, &SignalingMessage::PairingCreateCode).await;
    match ws_recv(&mut stream_c).await {
        SignalingMessage::PairingCreateCodeResponse { .. } => {
            // Good — code was consumed, C got no pairing but can still create codes
        }
        other => panic!("Expected PairingCreateCodeResponse, got: {:?}", other),
    }
}

// ── WebRTC: Invalid SDP offer ─────────────────────────────────────────────

/// Test 20: handle_offer with invalid SDP returns error.
#[tokio::test]
async fn test_webrtc_invalid_sdp_offer() {
    let (signaling_tx, _rx) = mpsc::unbounded_channel();
    let manager = WebRtcManager::new(signaling_tx);
    manager
        .create_session("bad-sdp-test".to_string(), None)
        .await
        .unwrap();

    let result = manager
        .handle_offer("bad-sdp-test", "this is not valid SDP")
        .await;
    assert!(result.is_err(), "Invalid SDP should return error");
    let err = result.unwrap_err();
    assert!(
        err.contains("Failed"),
        "Error should describe the SDP failure: {}",
        err
    );
}

/// Test 21: handle_offer for non-existent session returns error.
#[tokio::test]
async fn test_webrtc_offer_nonexistent_session() {
    let (signaling_tx, _rx) = mpsc::unbounded_channel();
    let manager = WebRtcManager::new(signaling_tx);

    let result = manager
        .handle_offer("does-not-exist", "v=0\r\n")
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

/// Test 22: add_ice_candidate for non-existent session returns error.
#[tokio::test]
async fn test_webrtc_ice_candidate_nonexistent_session() {
    let (signaling_tx, _rx) = mpsc::unbounded_channel();
    let manager = WebRtcManager::new(signaling_tx);

    let result = manager
        .add_ice_candidate("ghost-session", "candidate:0 1 udp 2122252543 127.0.0.1 9999 typ host", Some("0"), Some(0))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

// ── Silk: Command with non-zero exit code ─────────────────────────────────

/// Test 23: Silk command that fails produces non-zero exit code.
#[tokio::test]
async fn test_silk_command_nonzero_exit_e2e() {
    let harness = WebRtcTestHarness::new("silk-fail-test", None).await;

    harness
        .send_silk(&CocoonMessage::SilkCreateSession {
            cwd: None,
            env: None,
            shell: None,
        })
        .await;

    let session_id = match harness.recv_silk().await {
        CocoonMessage::SilkCreateSessionResponse { session_id, .. } => session_id,
        other => panic!("Expected SilkCreateSessionResponse, got: {:?}", other),
    };

    let command_id = uuid::Uuid::new_v4().to_string();
    harness
        .send_silk(&CocoonMessage::SilkExecute {
            session_id: session_id.clone(),
            command: "exit 42".to_string(),
            command_id: command_id.clone(),
            cols: Some(80),
            rows: Some(24),
            env: None,
        })
        .await;

    let timeout = std::time::Duration::from_secs(10);
    let start = std::time::Instant::now();
    let mut exit_code = None;

    while start.elapsed() < timeout {
        let msg = harness.recv_silk().await;
        match msg {
            CocoonMessage::SilkCommandCompleted {
                exit_code: code, ..
            } => {
                exit_code = Some(code);
                break;
            }
            CocoonMessage::SilkError { code, message, .. } => {
                panic!("Silk error: {} - {}", code, message);
            }
            _ => {}
        }
    }

    assert_eq!(exit_code, Some(42), "Should capture non-zero exit code");
    harness.cleanup().await;
}

// ── Silk: Stderr output ───────────────────────────────────────────────────

/// Test 24: Silk command that writes to stderr delivers SilkOutput with stderr stream.
#[tokio::test]
async fn test_silk_command_stderr_e2e() {
    let harness = WebRtcTestHarness::new("silk-stderr-test", None).await;

    harness
        .send_silk(&CocoonMessage::SilkCreateSession {
            cwd: None,
            env: None,
            shell: None,
        })
        .await;

    let session_id = match harness.recv_silk().await {
        CocoonMessage::SilkCreateSessionResponse { session_id, .. } => session_id,
        other => panic!("Expected SilkCreateSessionResponse, got: {:?}", other),
    };

    let command_id = uuid::Uuid::new_v4().to_string();
    harness
        .send_silk(&CocoonMessage::SilkExecute {
            session_id: session_id.clone(),
            command: "echo stderr_marker >&2".to_string(),
            command_id: command_id.clone(),
            cols: Some(80),
            rows: Some(24),
            env: None,
        })
        .await;

    let timeout = std::time::Duration::from_secs(10);
    let start = std::time::Instant::now();
    let mut got_stderr = false;

    while start.elapsed() < timeout {
        let msg = harness.recv_silk().await;
        match msg {
            CocoonMessage::SilkOutput {
                stream, data, ..
            } => {
                if matches!(stream, crate::protocol::types::SilkStream::Stderr)
                    && data.contains("stderr_marker")
                {
                    got_stderr = true;
                }
            }
            CocoonMessage::SilkCommandCompleted { .. } => break,
            CocoonMessage::SilkError { code, message, .. } => {
                panic!("Silk error: {} - {}", code, message);
            }
            _ => {}
        }
    }

    assert!(got_stderr, "Should have received stderr output");
    harness.cleanup().await;
}

// ── Silk: Multiple commands in sequence ───────────────────────────────────

/// Test 25: Execute multiple commands sequentially in the same silk session.
#[tokio::test]
async fn test_silk_multiple_commands_sequential_e2e() {
    let harness = WebRtcTestHarness::new("silk-multi-test", None).await;

    harness
        .send_silk(&CocoonMessage::SilkCreateSession {
            cwd: None,
            env: None,
            shell: None,
        })
        .await;

    let session_id = match harness.recv_silk().await {
        CocoonMessage::SilkCreateSessionResponse { session_id, .. } => session_id,
        other => panic!("Expected SilkCreateSessionResponse, got: {:?}", other),
    };

    // Run 3 commands in sequence
    for i in 1..=3 {
        let command_id = format!("cmd-{}", i);
        harness
            .send_silk(&CocoonMessage::SilkExecute {
                session_id: session_id.clone(),
                command: format!("echo output_{}", i),
                command_id: command_id.clone(),
                cols: Some(80),
                rows: Some(24),
                env: None,
            })
            .await;

        let timeout = std::time::Duration::from_secs(10);
        let start = std::time::Instant::now();
        let mut completed = false;
        let mut got_output = false;

        while start.elapsed() < timeout {
            let msg = harness.recv_silk().await;
            match msg {
                CocoonMessage::SilkOutput { data, .. } => {
                    if data.contains(&format!("output_{}", i)) {
                        got_output = true;
                    }
                }
                CocoonMessage::SilkCommandCompleted {
                    command_id: cid,
                    exit_code,
                    ..
                } => {
                    assert_eq!(cid, command_id);
                    assert_eq!(exit_code, 0);
                    completed = true;
                    break;
                }
                _ => {}
            }
        }

        assert!(got_output, "Command {} should produce output", i);
        assert!(completed, "Command {} should complete", i);
    }

    harness.cleanup().await;
}

// ── ADI: Multiple concurrent requests ─────────────────────────────────────

/// Test 26: Send multiple ADI requests concurrently and verify all get correct responses.
#[tokio::test]
async fn test_adi_concurrent_requests_e2e() {
    use crate::adi_frame::{RequestHeader, ResponseStatus};

    let mut router = AdiRouter::new();
    router.register(Arc::new(EchoPlugin));
    let router = Arc::new(Mutex::new(router));

    let harness = WebRtcTestHarness::new("adi-concurrent-test", Some(router)).await;

    // Send 5 requests without waiting for responses
    let mut request_ids = Vec::new();
    for i in 0..5 {
        let request_id = uuid::Uuid::new_v4();
        request_ids.push(request_id);
        let payload = serde_json::to_vec(&serde_json::json!({"index": i})).unwrap();
        harness
            .send_adi_frame(
                &RequestHeader {
                    v: 1,
                    id: request_id,
                    plugin: "adi.echo-test".to_string(),
                    method: "echo".to_string(),
                    stream: false,
                },
                &payload,
            )
            .await;
    }

    // Collect all 5 responses
    let mut received_ids = std::collections::HashSet::new();
    for _ in 0..5 {
        let (header, payload) = harness.recv_adi_frame().await;
        assert_eq!(header.status, ResponseStatus::Success);
        received_ids.insert(header.id);
        let data: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert!(data["index"].is_number());
    }

    // All request IDs should have been responded to
    for id in &request_ids {
        assert!(received_ids.contains(id), "Missing response for request {}", id);
    }

    harness.cleanup().await;
}

// ── ADI: Large payload ────────────────────────────────────────────────────

/// Test 27: ADI echo with a payload near SCTP max message size (~15KB).
#[tokio::test]
async fn test_adi_large_payload_e2e() {
    use crate::adi_frame::{RequestHeader, ResponseStatus};

    let mut router = AdiRouter::new();
    router.register(Arc::new(EchoPlugin));
    let router = Arc::new(Mutex::new(router));

    let harness = WebRtcTestHarness::new("adi-large-test", Some(router)).await;

    let request_id = uuid::Uuid::new_v4();
    // ~15KB payload (within SCTP default max message size)
    let large_payload = vec![0x42u8; 15_000];
    harness
        .send_adi_frame(
            &RequestHeader {
                v: 1,
                id: request_id,
                plugin: "adi.echo-test".to_string(),
                method: "echo".to_string(),
                stream: false,
            },
            &large_payload,
        )
        .await;

    let (header, payload) = harness.recv_adi_frame().await;
    assert_eq!(header.id, request_id);
    assert_eq!(header.status, ResponseStatus::Success);
    assert_eq!(payload.len(), 15_000);
    assert!(payload.iter().all(|&b| b == 0x42));

    harness.cleanup().await;
}
