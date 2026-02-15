use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

#[derive(Debug, Serialize, Deserialize)]
struct SignalMessage {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "roomId", default)]
    room_id: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    data: Option<Value>,
    #[serde(default)]
    reason: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let signaling_url = std::env::var("SIGNALING_URL").unwrap_or_else(|_| "ws://127.0.0.1:8787".to_string());
    let room_id = std::env::var("ROOM_ID").unwrap_or_else(|_| "demo-room".to_string());

    println!("[receiver] signaling={signaling_url} room={room_id}");

    let pc = build_peer_connection().await?;
    let (ws_stream, _) = connect_async(&signaling_url)
        .await
        .with_context(|| format!("failed to connect signaling: {signaling_url}"))?;

    println!("[receiver] signaling connected");

    let (mut ws_write, mut ws_read) = ws_stream.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();

    let writer = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if ws_write.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    });

    send_signal(
        &out_tx,
        SignalMessage {
            kind: "join".to_string(),
            room_id: Some(room_id.clone()),
            role: Some("host".to_string()),
            from: None,
            data: None,
            reason: None,
        },
    )?;

    wire_ice_callback(pc.clone(), out_tx.clone(), room_id.clone()).await;
    wire_track_callback(pc.clone()).await;

    while let Some(msg) = ws_read.next().await {
        let msg = msg?;
        if !msg.is_text() {
            continue;
        }

        let incoming: SignalMessage = serde_json::from_str(msg.to_text()?)
            .context("failed to parse signaling message")?;

        match incoming.kind.as_str() {
            "connected" => println!("[receiver] connected id assigned"),
            "joined" => println!("[receiver] joined as host"),
            "peer-ready" => println!("[receiver] sender is ready"),
            "offer" => {
                let raw = incoming
                    .data
                    .ok_or_else(|| anyhow!("offer missing data"))?;
                let offer: RTCSessionDescription = serde_json::from_value(raw)?;

                pc.set_remote_description(offer).await?;
                let answer = pc.create_answer(None).await?;
                pc.set_local_description(answer).await?;

                tokio::time::timeout(Duration::from_secs(3), pc.gathering_complete_promise())
                    .await
                    .ok();

                let local = pc
                    .local_description()
                    .await
                    .ok_or_else(|| anyhow!("local description unavailable"))?;

                send_signal(
                    &out_tx,
                    SignalMessage {
                        kind: "answer".to_string(),
                        room_id: Some(room_id.clone()),
                        role: None,
                        from: Some("host".to_string()),
                        data: Some(serde_json::to_value(local)?),
                        reason: None,
                    },
                )?;
                println!("[receiver] answer sent");
            }
            "ice" => {
                if let Some(raw) = incoming.data {
                    let candidate: RTCIceCandidateInit = serde_json::from_value(raw)?;
                    pc.add_ice_candidate(candidate).await?;
                }
            }
            "peer-left" => {
                println!("[receiver] sender left room");
            }
            "error" => {
                println!(
                    "[receiver] signaling error: {}",
                    incoming.reason.unwrap_or_else(|| "unknown".to_string())
                );
            }
            _ => {}
        }
    }

    writer.abort();
    println!("[receiver] signaling disconnected");
    Ok(())
}

async fn build_peer_connection() -> Result<Arc<RTCPeerConnection>> {
    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs()?;

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut media_engine)?;

    let api = APIBuilder::new()
        .with_media_engine(media_engine)
        .with_interceptor_registry(registry)
        .build();

    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            ..Default::default()
        }],
        ..Default::default()
    };

    Ok(Arc::new(api.new_peer_connection(config).await?))
}

async fn wire_ice_callback(pc: Arc<RTCPeerConnection>, out_tx: mpsc::UnboundedSender<String>, room_id: String) {
    pc.on_ice_candidate(Box::new(move |candidate| {
        let out_tx = out_tx.clone();
        let room_id = room_id.clone();

        Box::pin(async move {
            if let Some(candidate) = candidate {
                if let Ok(init) = candidate.to_json() {
                    let _ = send_signal(
                        &out_tx,
                        SignalMessage {
                            kind: "ice".to_string(),
                            room_id: Some(room_id),
                            role: None,
                            from: Some("host".to_string()),
                            data: serde_json::to_value(init).ok(),
                            reason: None,
                        },
                    );
                }
            }
        })
    }));
}

async fn wire_track_callback(pc: Arc<RTCPeerConnection>) {
    pc.on_track(Box::new(move |track, _, _| {
        Box::pin(async move {
            println!(
                "[receiver] track received kind={} codec={} ssrc={}",
                track.kind(),
                track.codec().capability.mime_type,
                track.ssrc()
            );

            let mut packets: u64 = 0;
            let mut last = Instant::now();

            loop {
                match track.read_rtp().await {
                    Ok((_packet, _)) => {
                        packets += 1;
                        if last.elapsed() >= Duration::from_secs(1) {
                            println!("[receiver] inbound rtp packets/sec={packets}");
                            packets = 0;
                            last = Instant::now();
                        }
                    }
                    Err(err) => {
                        println!("[receiver] track ended: {err}");
                        break;
                    }
                }
            }
        })
    }));
}

fn send_signal(out_tx: &mpsc::UnboundedSender<String>, msg: SignalMessage) -> Result<()> {
    let text = serde_json::to_string(&msg)?;
    out_tx
        .send(text)
        .map_err(|_| anyhow!("failed to enqueue signaling message"))
}
