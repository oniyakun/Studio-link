use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use futures_util::{SinkExt, StreamExt};
use opus::{Channels, Decoder as OpusDecoder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
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
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;

const OPUS_SAMPLE_RATE: u32 = 48_000;
const MAX_OPUS_SAMPLES_PER_PACKET: usize = 5_760;
const MAX_BUFFER_SECONDS: usize = 5;

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

struct AudioSink {
    sample_rate: u32,
    queue: Arc<Mutex<VecDeque<f32>>>,
}

impl AudioSink {
    fn push_mono_i16(&self, input: &[i16], input_sample_rate: u32) {
        if input.is_empty() {
            return;
        }

        let mono_f32 = if input_sample_rate == self.sample_rate {
            input.iter().map(|s| *s as f32 / i16::MAX as f32).collect::<Vec<f32>>()
        } else {
            resample_mono_i16(input, input_sample_rate, self.sample_rate)
                .into_iter()
                .map(|s| s as f32 / i16::MAX as f32)
                .collect::<Vec<f32>>()
        };

        let max_samples = (self.sample_rate as usize) * MAX_BUFFER_SECONDS;

        if let Ok(mut queue) = self.queue.lock() {
            queue.extend(mono_f32);

            if queue.len() > max_samples {
                let overflow = queue.len() - max_samples;
                queue.drain(..overflow);
            }
        }
    }
}

fn start_audio_output(device_name_filter: Option<&str>) -> Result<(Arc<AudioSink>, Stream)> {
        let host = cpal::default_host();
        let device = select_output_device(&host, device_name_filter)?;
        let device_name = device
            .name()
            .unwrap_or_else(|_| "unknown-output-device".to_string());

        let default_config = device
            .default_output_config()
            .context("failed to get default output config")?;

        let sample_rate = default_config.sample_rate().0;
        let channels = default_config.channels();
        let stream_config = default_config.config();
        let queue = Arc::new(Mutex::new(VecDeque::<f32>::with_capacity(
            (sample_rate as usize) * MAX_BUFFER_SECONDS,
        )));

        let queue_for_cb = queue.clone();
        let err_fn = |err| eprintln!("[audio] stream error: {err}");

        let stream = match default_config.sample_format() {
            SampleFormat::F32 => device.build_output_stream(
                &stream_config,
                move |data: &mut [f32], _| fill_output_f32(data, channels as usize, &queue_for_cb),
                err_fn,
                None,
            )?,
            SampleFormat::I16 => device.build_output_stream(
                &stream_config,
                move |data: &mut [i16], _| fill_output_i16(data, channels as usize, &queue_for_cb),
                err_fn,
                None,
            )?,
            SampleFormat::U16 => device.build_output_stream(
                &stream_config,
                move |data: &mut [u16], _| fill_output_u16(data, channels as usize, &queue_for_cb),
                err_fn,
                None,
            )?,
            _ => return Err(anyhow!("unsupported sample format")),
        };

        stream.play().context("failed to start output stream")?;

        println!(
            "[audio] output device='{}' sample_rate={} channels={}",
            device_name, sample_rate, channels
        );

        let sink = Arc::new(AudioSink {
            sample_rate,
            queue,
        });
        Ok((sink, stream))
}

#[tokio::main]
async fn main() -> Result<()> {
    let signaling_url = std::env::var("SIGNALING_URL").unwrap_or_else(|_| "ws://127.0.0.1:8787".to_string());
    let room_id = std::env::var("ROOM_ID").unwrap_or_else(|_| "demo-room".to_string());
    let output_name_filter = std::env::var("VIRTUAL_MIC_OUTPUT_NAME").ok();

    println!("[receiver] signaling={signaling_url} room={room_id}");

    let (audio_sink, _audio_stream) = start_audio_output(output_name_filter.as_deref())?;

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

    wire_ice_callback(pc.clone(), out_tx.clone(), room_id.clone());
    wire_track_callback(pc.clone(), audio_sink);

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
                let raw = incoming.data.ok_or_else(|| anyhow!("offer missing data"))?;
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

fn wire_ice_callback(pc: Arc<RTCPeerConnection>, out_tx: mpsc::UnboundedSender<String>, room_id: String) {
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

fn wire_track_callback(pc: Arc<RTCPeerConnection>, audio_output: Arc<AudioSink>) {
    pc.on_track(Box::new(move |track, _, _| {
        let audio_output = audio_output.clone();

        Box::pin(async move {
            let codec = track.codec().capability.mime_type;
            println!(
                "[receiver] track received kind={} codec={} ssrc={}",
                track.kind(),
                codec,
                track.ssrc()
            );

            if track.kind() != RTPCodecType::Audio {
                println!("[receiver] non-audio track ignored in phase 1");
                return;
            }

            if !codec.eq_ignore_ascii_case("audio/opus") {
                println!("[receiver] unsupported audio codec={} (expected audio/opus)", codec);
                return;
            }

            let mut decoder = match OpusDecoder::new(OPUS_SAMPLE_RATE, Channels::Mono) {
                Ok(d) => d,
                Err(err) => {
                    eprintln!("[audio] opus decoder init failed: {err}");
                    return;
                }
            };

            let mut packet_count: u64 = 0;
            let mut decode_errors: u64 = 0;
            let mut last = Instant::now();
            let mut pcm = vec![0i16; MAX_OPUS_SAMPLES_PER_PACKET];

            loop {
                match track.read_rtp().await {
                    Ok((packet, _)) => {
                        packet_count += 1;

                        if packet.payload.is_empty() {
                            continue;
                        }

                        match decoder.decode(&packet.payload, &mut pcm, false) {
                            Ok(decoded_samples) => {
                                audio_output.push_mono_i16(&pcm[..decoded_samples], OPUS_SAMPLE_RATE);
                            }
                            Err(err) => {
                                decode_errors += 1;
                                if decode_errors % 30 == 1 {
                                    eprintln!("[audio] opus decode error: {err}");
                                }
                            }
                        }

                        if last.elapsed() >= Duration::from_secs(1) {
                            println!(
                                "[receiver] inbound rtp packets/sec={} decode_errors_total={}",
                                packet_count, decode_errors
                            );
                            packet_count = 0;
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

fn select_output_device(host: &cpal::Host, filter: Option<&str>) -> Result<cpal::Device> {
    if let Some(raw_filter) = filter {
        let filter = raw_filter.trim().to_lowercase();
        if !filter.is_empty() {
            for device in host.output_devices().context("failed to enumerate output devices")? {
                let name = device.name().unwrap_or_default();
                if name.to_lowercase().contains(&filter) {
                    return Ok(device);
                }
            }
            return Err(anyhow!(
                "no output device matched VIRTUAL_MIC_OUTPUT_NAME='{}'",
                raw_filter
            ));
        }
    }

    host.default_output_device()
        .ok_or_else(|| anyhow!("no default output device found"))
}

fn fill_output_f32(data: &mut [f32], channels: usize, queue: &Arc<Mutex<VecDeque<f32>>>) {
    let mut guard = match queue.lock() {
        Ok(g) => g,
        Err(_) => {
            data.fill(0.0);
            return;
        }
    };

    for frame in data.chunks_mut(channels) {
        let sample = guard.pop_front().unwrap_or(0.0);
        for out in frame.iter_mut() {
            *out = sample;
        }
    }
}

fn fill_output_i16(data: &mut [i16], channels: usize, queue: &Arc<Mutex<VecDeque<f32>>>) {
    let mut guard = match queue.lock() {
        Ok(g) => g,
        Err(_) => {
            data.fill(0);
            return;
        }
    };

    for frame in data.chunks_mut(channels) {
        let sample = guard.pop_front().unwrap_or(0.0).clamp(-1.0, 1.0);
        let s = (sample * i16::MAX as f32) as i16;
        for out in frame.iter_mut() {
            *out = s;
        }
    }
}

fn fill_output_u16(data: &mut [u16], channels: usize, queue: &Arc<Mutex<VecDeque<f32>>>) {
    let mut guard = match queue.lock() {
        Ok(g) => g,
        Err(_) => {
            data.fill(u16::MAX / 2);
            return;
        }
    };

    for frame in data.chunks_mut(channels) {
        let sample = guard.pop_front().unwrap_or(0.0).clamp(-1.0, 1.0);
        let scaled = ((sample * 0.5) + 0.5) * u16::MAX as f32;
        let s = scaled as u16;
        for out in frame.iter_mut() {
            *out = s;
        }
    }
}

fn resample_mono_i16(input: &[i16], src_rate: u32, dst_rate: u32) -> Vec<i16> {
    if input.is_empty() || src_rate == 0 || dst_rate == 0 {
        return Vec::new();
    }

    if src_rate == dst_rate {
        return input.to_vec();
    }

    let dst_len = ((input.len() as u64) * (dst_rate as u64) / (src_rate as u64)) as usize;
    if dst_len == 0 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(dst_len);
    let ratio = src_rate as f64 / dst_rate as f64;

    for i in 0..dst_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;

        let s0 = input.get(idx).copied().unwrap_or(0) as f32;
        let s1 = input.get(idx + 1).copied().unwrap_or(s0 as i16) as f32;
        let sample = s0 + (s1 - s0) * frac;
        out.push(sample as i16);
    }

    out
}
