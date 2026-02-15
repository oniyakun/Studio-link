use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use futures_util::{SinkExt, StreamExt};
use openh264::decoder::Decoder as H264Decoder;
use openh264::formats::YUVSource;
use opus::{Channels, Decoder as OpusDecoder};
use rtp::codecs::h264::H264Packet;
use rtp::packetizer::Depacketizer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc as std_mpsc};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use virtualcam::{Camera as VirtualCamera, PixelFormat as VirtualPixelFormat};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtcp::packet::Packet as RtcpPacket;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::rtp_transceiver::RTCRtpTransceiverInit;

const OPUS_SAMPLE_RATE: u32 = 48_000;
const MAX_OPUS_SAMPLES_PER_PACKET: usize = 5_760;
const MAX_BUFFER_SECONDS: usize = 5;
const VIDEO_DEFAULT_FPS: u32 = 30;
const FIXED_ROOM_ID: &str = "local-session";

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

#[derive(Clone)]
struct VideoRelayConfig {
    enable_virtual_cam: bool,
    virtual_cam_backend: String,
}

impl AudioSink {
    fn push_mono_i16(&self, input: &[i16], input_sample_rate: u32) {
        if input.is_empty() {
            return;
        }

        let mono_f32 = if input_sample_rate == self.sample_rate {
            input
                .iter()
                .map(|s| *s as f32 / i16::MAX as f32)
                .collect::<Vec<f32>>()
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

    let sink = Arc::new(AudioSink { sample_rate, queue });
    Ok((sink, stream))
}

#[tokio::main]
async fn main() -> Result<()> {
    let signaling_url = std::env::var("SIGNALING_URL").unwrap_or_else(|_| "ws://127.0.0.1:8787".to_string());
    let room_id = FIXED_ROOM_ID.to_string();
    let output_name_filter = std::env::var("VIRTUAL_MIC_OUTPUT_NAME").ok();
    let enable_virtual_cam = std::env::var("VIRTUAL_CAM_ENABLED")
        .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"))
        .unwrap_or(true);
    let virtual_cam_backend =
        std::env::var("VIRTUAL_CAM_BACKEND").unwrap_or_else(|_| "obs".to_string());
    let video_relay_config = VideoRelayConfig {
        enable_virtual_cam,
        virtual_cam_backend,
    };

    println!("[receiver] signaling={signaling_url}");
    println!(
        "[video] virtual_cam enabled={} backend={}",
        video_relay_config.enable_virtual_cam, video_relay_config.virtual_cam_backend
    );

    let (audio_sink, _audio_stream) = start_audio_output(output_name_filter.as_deref())?;

    let (ws_stream, _) = connect_async(&signaling_url)
        .await
        .with_context(|| format!("failed to connect signaling: {signaling_url}"))?;

    println!("[receiver] signaling connected");

    let (mut ws_write, mut ws_read) = ws_stream.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let mut pending_remote_ice: Vec<RTCIceCandidateInit> = Vec::new();

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
    let mut pc = create_wired_peer_connection(
        audio_sink.clone(),
        video_relay_config.clone(),
        out_tx.clone(),
        room_id.clone(),
    )
    .await?;

    while let Some(msg) = ws_read.next().await {
        let msg = msg?;
        if !msg.is_text() {
            continue;
        }

        let incoming: SignalMessage =
            serde_json::from_str(msg.to_text()?).context("failed to parse signaling message")?;

        match incoming.kind.as_str() {
            "connected" => println!("[receiver] connected id assigned"),
            "joined" => println!("[receiver] joined as host"),
            "peer-ready" => println!("[receiver] sender is ready"),
            "offer" => {
                if pc.remote_description().await.is_some() {
                    println!("[receiver] previous session detected, rebuilding peer connection");
                    if let Err(err) = pc.close().await {
                        eprintln!("[receiver] close old peer connection error: {err}");
                    }
                    pending_remote_ice.clear();
                    pc = create_wired_peer_connection(
                        audio_sink.clone(),
                        video_relay_config.clone(),
                        out_tx.clone(),
                        room_id.clone(),
                    )
                    .await?;
                }

                let raw = incoming.data.ok_or_else(|| anyhow!("offer missing data"))?;
                let offer: RTCSessionDescription = serde_json::from_value(raw)?;
                let sdp_text = offer.sdp.clone();
                let video_m_lines = sdp_text.matches("\nm=video ").count()
                    + if sdp_text.starts_with("m=video ") { 1 } else { 0 };
                let audio_m_lines = sdp_text.matches("\nm=audio ").count()
                    + if sdp_text.starts_with("m=audio ") { 1 } else { 0 };
                println!(
                    "[receiver] offer summary audio_m={} video_m={} h264={} vp8={} vp9={}",
                    audio_m_lines,
                    video_m_lines,
                    sdp_text.contains("H264/90000"),
                    sdp_text.contains("VP8/90000"),
                    sdp_text.contains("VP9/90000")
                );

                pc.set_remote_description(offer).await?;
                log_transceivers(&pc, "after_set_remote").await;
                if !pending_remote_ice.is_empty() {
                    for candidate in pending_remote_ice.drain(..) {
                        if let Err(err) = pc.add_ice_candidate(candidate).await {
                            eprintln!("[receiver] buffered ICE apply error: {err}");
                        }
                    }
                }
                let answer = pc.create_answer(None).await?;
                pc.set_local_description(answer).await?;
                log_transceivers(&pc, "after_set_local").await;

                tokio::time::timeout(Duration::from_secs(3), pc.gathering_complete_promise())
                    .await
                    .ok();

                let local = pc
                    .local_description()
                    .await
                    .ok_or_else(|| anyhow!("local description unavailable"))?;
                let ans_sdp = local.sdp.clone();
                let ans_video_m_lines = ans_sdp.matches("\nm=video ").count()
                    + if ans_sdp.starts_with("m=video ") { 1 } else { 0 };
                let ans_audio_m_lines = ans_sdp.matches("\nm=audio ").count()
                    + if ans_sdp.starts_with("m=audio ") { 1 } else { 0 };
                let video_rejected = ans_sdp.contains("m=video 0 ");
                let video_inactive = ans_sdp.contains("\na=inactive")
                    || ans_sdp.starts_with("a=inactive");
                println!(
                    "[receiver] answer summary audio_m={} video_m={} video_rejected={} video_inactive={}",
                    ans_audio_m_lines, ans_video_m_lines, video_rejected, video_inactive
                );

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
                    if pc.remote_description().await.is_none() {
                        pending_remote_ice.push(candidate);
                    } else if let Err(err) = pc.add_ice_candidate(candidate).await {
                        eprintln!("[receiver] ICE apply error: {err}");
                    }
                }
            }
            "peer-left" => {
                println!("[receiver] sender disconnected");
                if let Err(err) = pc.close().await {
                    eprintln!("[receiver] close peer connection on peer-left error: {err}");
                }
                pending_remote_ice.clear();
                pc = create_wired_peer_connection(
                    audio_sink.clone(),
                    video_relay_config.clone(),
                    out_tx.clone(),
                    room_id.clone(),
                )
                .await?;
                println!("[receiver] peer connection reset and ready for next sender");
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

async fn create_wired_peer_connection(
    audio_sink: Arc<AudioSink>,
    video_relay_config: VideoRelayConfig,
    out_tx: mpsc::UnboundedSender<String>,
    room_id: String,
) -> Result<Arc<RTCPeerConnection>> {
    let pc = build_peer_connection().await?;
    let pc_for_state = pc.clone();
    pc.on_peer_connection_state_change(Box::new(move |s| {
        let pc_for_state = pc_for_state.clone();
        Box::pin(async move {
            println!("[receiver] pc state={s}");
            if s.to_string().eq_ignore_ascii_case("connected") {
                log_transceivers(&pc_for_state, "on_connected").await;
            }
        })
    }));
    pc.on_ice_connection_state_change(Box::new(move |s| {
        println!("[receiver] ice state={s}");
        Box::pin(async {})
    }));

    wire_ice_callback(pc.clone(), out_tx, room_id);
    wire_track_callback(pc.clone(), audio_sink, video_relay_config);
    Ok(pc)
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

    let pc = Arc::new(api.new_peer_connection(config).await?);

    // Pre-declare recvonly transceivers so remote audio/video tracks are consistently bound.
    let recvonly_audio = Some(RTCRtpTransceiverInit {
        direction: RTCRtpTransceiverDirection::Recvonly,
        send_encodings: vec![],
    });
    let recvonly_video = Some(RTCRtpTransceiverInit {
        direction: RTCRtpTransceiverDirection::Recvonly,
        send_encodings: vec![],
    });
    pc.add_transceiver_from_kind(RTPCodecType::Audio, recvonly_audio)
        .await
        .context("failed to add audio recvonly transceiver")?;
    pc.add_transceiver_from_kind(RTPCodecType::Video, recvonly_video)
        .await
        .context("failed to add video recvonly transceiver")?;

    Ok(pc)
}

fn wire_ice_callback(
    pc: Arc<RTCPeerConnection>,
    out_tx: mpsc::UnboundedSender<String>,
    room_id: String,
) {
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

fn wire_track_callback(pc: Arc<RTCPeerConnection>, audio_sink: Arc<AudioSink>, video_relay_config: VideoRelayConfig) {
    let pc_for_cb = pc.clone();
    pc.on_track(Box::new(move |track, _, _| {
        let audio_sink = audio_sink.clone();
        let video_relay_config = video_relay_config.clone();
        let pc_for_track = pc_for_cb.clone();

        Box::pin(async move {
            tokio::spawn(async move {
                let codec = track.codec().capability.mime_type;
                println!(
                    "[receiver] track received kind={} codec={} ssrc={}",
                    track.kind(),
                    codec,
                    track.ssrc()
                );

                match track.kind() {
                    RTPCodecType::Audio => handle_audio_track(track, codec, audio_sink).await,
                    RTPCodecType::Video => {
                        handle_video_track(track, codec, video_relay_config, pc_for_track).await
                    }
                    _ => println!("[receiver] unsupported track kind"),
                }
            });
        })
    }));
}

async fn handle_audio_track(
    track: Arc<webrtc::track::track_remote::TrackRemote>,
    codec: String,
    audio_sink: Arc<AudioSink>,
) {
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
                        audio_sink.push_mono_i16(&pcm[..decoded_samples], OPUS_SAMPLE_RATE);
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
                        "[audio] inbound packets/sec={} decode_errors_total={}",
                        packet_count, decode_errors
                    );
                    packet_count = 0;
                    last = Instant::now();
                }
            }
            Err(err) => {
                println!("[audio] track ended: {err}");
                break;
            }
        }
    }
}

async fn handle_video_track(
    track: Arc<webrtc::track::track_remote::TrackRemote>,
    codec: String,
    video_relay_config: VideoRelayConfig,
    pc: Arc<RTCPeerConnection>,
) {
    let media_ssrc = track.ssrc();
    let mut last_pli = Instant::now()
        .checked_sub(Duration::from_secs(5))
        .unwrap_or_else(Instant::now);
    if let Err(err) = request_pli(&pc, media_ssrc, "track-start").await {
        eprintln!("[video] initial PLI request failed: {err}");
    }

    let virtual_cam_tx = if video_relay_config.enable_virtual_cam
        && codec.eq_ignore_ascii_case("video/h264")
    {
        Some(start_virtual_cam_worker(
            track.ssrc(),
            codec.clone(),
            video_relay_config.clone(),
        ))
    } else {
        if video_relay_config.enable_virtual_cam {
            println!("[video] virtual cam currently supports H264 only; codec={} will skip", codec);
        }
        None
    };
    if video_relay_config.enable_virtual_cam && virtual_cam_tx.is_some() {
        println!("[video] virtual cam pipeline armed for codec={}", codec);
    }

    let mut packet_count: u64 = 0;
    let mut last = Instant::now();

    loop {
        match track.read_rtp().await {
            Ok((packet, _)) => {
                packet_count += 1;
                if last_pli.elapsed() >= Duration::from_secs(2) {
                    if let Err(err) = request_pli(&pc, media_ssrc, "periodic").await {
                        eprintln!("[video] periodic PLI request failed: {err}");
                    }
                    last_pli = Instant::now();
                }
                if let Some(tx) = &virtual_cam_tx {
                    let _ = tx.send(packet.clone());
                }

                if last.elapsed() >= Duration::from_secs(1) {
                    println!("[video] inbound packets/sec={}", packet_count);
                    packet_count = 0;
                    last = Instant::now();
                }
            }
            Err(err) => {
                println!("[video] track ended: {err}");
                break;
            }
        }
    }
}

async fn request_pli(pc: &Arc<RTCPeerConnection>, media_ssrc: u32, reason: &str) -> Result<()> {
    pc.write_rtcp(&[Box::new(PictureLossIndication {
        sender_ssrc: 0,
        media_ssrc,
    }) as Box<dyn RtcpPacket + Send + Sync>])
    .await
    .map(|_| ())
    .with_context(|| format!("failed to request PLI ({reason})"))
}

fn start_virtual_cam_worker(
    ssrc: u32,
    codec: String,
    video_relay_config: VideoRelayConfig,
) -> std_mpsc::Sender<rtp::packet::Packet> {
    let (tx, rx) = std_mpsc::channel::<rtp::packet::Packet>();
    thread::spawn(move || {
        if let Err(err) = run_virtual_cam_worker(ssrc, &codec, &video_relay_config, rx) {
            eprintln!("[video] virtual cam worker ended with error: {err}");
        }
    });
    tx
}

fn run_virtual_cam_worker(
    ssrc: u32,
    codec: &str,
    video_relay_config: &VideoRelayConfig,
    rx: std_mpsc::Receiver<rtp::packet::Packet>,
) -> Result<()> {
    if !codec.eq_ignore_ascii_case("video/h264") {
        return Ok(());
    }

    let mut depacketizer = H264Packet::default();
    let mut decoder = H264Decoder::new().context("failed to create H264 decoder")?;
    let mut camera: Option<VirtualCamera> = None;
    let mut virtual_cam_errors: u64 = 0;
    let mut input_packets: u64 = 0;
    let mut decoded_frames: u64 = 0;
    let mut sent_frames: u64 = 0;
    let mut cam_dims: Option<(usize, usize)> = None;
    let mut access_unit: Vec<u8> = Vec::with_capacity(1024 * 1024);
    let mut last_seq: Option<u16> = None;
    let mut waiting_for_idr = false;
    let forced_cam_width = std::env::var("VIRTUAL_CAM_WIDTH")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0 && *v % 2 == 0);
    let forced_cam_height = std::env::var("VIRTUAL_CAM_HEIGHT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0 && *v % 2 == 0);
    let mut cached_sps: Option<Vec<u8>> = None;
    let mut cached_pps: Option<Vec<u8>> = None;
    let mut au_has_sps = false;
    let mut au_has_pps = false;

    for packet in rx {
        input_packets += 1;
        let seq = packet.header.sequence_number;
        if let Some(prev) = last_seq {
            let expected = prev.wrapping_add(1);
            if seq != expected {
                virtual_cam_errors += 1;
                waiting_for_idr = true;
                access_unit.clear();
                au_has_sps = false;
                au_has_pps = false;
                depacketizer = H264Packet::default();
                if virtual_cam_errors % 30 == 1 {
                    eprintln!("[video] RTP sequence gap prev={} current={} -> drop AU and wait IDR", prev, seq);
                }
            }
        }
        last_seq = Some(seq);

        let nal = match depacketizer.depacketize(&packet.payload) {
            Ok(p) => p,
            Err(err) => {
                virtual_cam_errors += 1;
                if virtual_cam_errors % 30 == 1 {
                    eprintln!("[video] depacketize error: {err}");
                }
                continue;
            }
        };

        if nal.is_empty() {
            continue;
        }

        let nal_type = first_h264_nal_type(&nal);
        if let Some(t) = nal_type {
            if t == 7 {
                cached_sps = Some(nal.to_vec());
                au_has_sps = true;
            } else if t == 8 {
                cached_pps = Some(nal.to_vec());
                au_has_pps = true;
            } else if t == 5 {
                waiting_for_idr = false;
                // Some streams do not repeat SPS/PPS before every IDR.
                // Prepend cached parameter sets when missing in this access unit.
                if !au_has_sps {
                    if let Some(sps) = cached_sps.as_ref() {
                        append_h264_annexb_nal(&mut access_unit, sps);
                        au_has_sps = true;
                    }
                }
                if !au_has_pps {
                    if let Some(pps) = cached_pps.as_ref() {
                        append_h264_annexb_nal(&mut access_unit, pps);
                        au_has_pps = true;
                    }
                }
            }
        }

        if waiting_for_idr && !matches!(nal_type, Some(7) | Some(8) | Some(5)) {
            continue;
        }

        append_h264_annexb_nal(&mut access_unit, &nal);

        // H264 over RTP may split one frame across multiple packets.
        // Decode only when marker bit says end of access unit.
        if !packet.header.marker {
            continue;
        }

        let decoded = match decoder.decode(&access_unit) {
            Ok(v) => v,
            Err(err) => {
                virtual_cam_errors += 1;
                if virtual_cam_errors % 30 == 1 {
                    eprintln!(
                        "[video] h264 decode error: {err} (packet_len={} au_len={})",
                        packet.payload.len(),
                        access_unit.len()
                    );
                }
                access_unit.clear();
                au_has_sps = false;
                au_has_pps = false;
                waiting_for_idr = true;
                continue;
            }
        };
        access_unit.clear();
        au_has_sps = false;
        au_has_pps = false;

        let Some(yuv) = decoded else {
            if input_packets % 300 == 0 {
                println!(
                    "[video] virtualcam waiting for decodable frame packets={} decoded={} sent={} errors={}",
                    input_packets, decoded_frames, sent_frames, virtual_cam_errors
                );
            }
            continue;
        };
        decoded_frames += 1;

        let (width, height) = yuv.dimensions();
        if width % 2 != 0 || height % 2 != 0 {
            continue;
        }

        if camera.is_none() {
            let (open_w, open_h) = match (forced_cam_width, forced_cam_height) {
                (Some(w), Some(h)) => (w, h),
                _ => (width, height),
            };
            let cam = VirtualCamera::builder(open_w as u32, open_h as u32, VIDEO_DEFAULT_FPS as f64)
                .format(VirtualPixelFormat::I420)
                .backend(video_relay_config.virtual_cam_backend.as_str())
                .build()
                .with_context(|| {
                    format!(
                        "failed to open virtual cam backend={} for {}x{}",
                        video_relay_config.virtual_cam_backend, open_w, open_h
                    )
                })?;
            println!(
                "[video] virtual cam opened backend={} device={} ssrc={}",
                cam.backend(),
                cam.device(),
                ssrc
            );
            camera = Some(cam);
            cam_dims = Some((open_w, open_h));
        }

        let frame_i420_src = pack_i420_from_yuv_slices(
            yuv.y(),
            yuv.u(),
            yuv.v(),
            yuv.strides(),
            width,
            height,
        );
        let frame_i420 = if let Some((cw, ch)) = cam_dims {
            if cw != width || ch != height {
                resize_i420_nearest(&frame_i420_src, width, height, cw, ch)
            } else {
                frame_i420_src
            }
        } else {
            frame_i420_src
        };

        if let Some(cam) = camera.as_mut() {
            if let Err(err) = cam.send(&frame_i420) {
                virtual_cam_errors += 1;
                if virtual_cam_errors % 30 == 1 {
                    eprintln!("[video] virtual cam send error: {err}");
                }
            } else {
                sent_frames += 1;
                if sent_frames % 120 == 0 {
                    println!(
                        "[video] virtualcam sent_frames={} decoded_frames={} packets={} errors={}",
                        sent_frames, decoded_frames, input_packets, virtual_cam_errors
                    );
                }
            }
        }
    }

    Ok(())
}

fn resize_i420_nearest(src: &[u8], src_w: usize, src_h: usize, dst_w: usize, dst_h: usize) -> Vec<u8> {
    let src_y_len = src_w * src_h;
    let src_uv_w = src_w / 2;
    let src_uv_h = src_h / 2;
    let src_u_len = src_uv_w * src_uv_h;
    let src_v_len = src_u_len;
    if src.len() < src_y_len + src_u_len + src_v_len {
        return vec![0u8; dst_w * dst_h + (dst_w / 2) * (dst_h / 2) * 2];
    }

    let (src_y, rest) = src.split_at(src_y_len);
    let (src_u, src_v) = rest.split_at(src_u_len);

    let dst_y_len = dst_w * dst_h;
    let dst_uv_w = dst_w / 2;
    let dst_uv_h = dst_h / 2;
    let dst_u_len = dst_uv_w * dst_uv_h;
    let dst_v_len = dst_u_len;
    let mut out = vec![0u8; dst_y_len + dst_u_len + dst_v_len];

    {
        let (dst_y, rest) = out.split_at_mut(dst_y_len);
        let (dst_u, dst_v) = rest.split_at_mut(dst_u_len);

        for y in 0..dst_h {
            let sy = y * src_h / dst_h;
            for x in 0..dst_w {
                let sx = x * src_w / dst_w;
                dst_y[y * dst_w + x] = src_y[sy * src_w + sx];
            }
        }

        for y in 0..dst_uv_h {
            let sy = y * src_uv_h / dst_uv_h;
            for x in 0..dst_uv_w {
                let sx = x * src_uv_w / dst_uv_w;
                dst_u[y * dst_uv_w + x] = src_u[sy * src_uv_w + sx];
                dst_v[y * dst_uv_w + x] = src_v[sy * src_uv_w + sx];
            }
        }
    }

    out
}

fn append_h264_annexb_nal(out: &mut Vec<u8>, nal: &[u8]) {
    if is_annexb_prefixed(nal) {
        out.extend_from_slice(nal);
    } else {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
}

fn is_annexb_prefixed(nal: &[u8]) -> bool {
    nal.len() >= 4 && nal[0] == 0 && nal[1] == 0 && (nal[2] == 1 || (nal[2] == 0 && nal[3] == 1))
}

fn first_h264_nal_type(buf: &[u8]) -> Option<u8> {
    if buf.is_empty() {
        return None;
    }

    if !is_annexb_prefixed(buf) {
        return Some(buf[0] & 0x1f);
    }

    let mut i = 0usize;
    while i + 3 < buf.len() {
        if buf[i] == 0 && buf[i + 1] == 0 {
            if buf[i + 2] == 1 {
                let h = i + 3;
                if h < buf.len() {
                    return Some(buf[h] & 0x1f);
                }
                return None;
            }
            if i + 4 < buf.len() && buf[i + 2] == 0 && buf[i + 3] == 1 {
                let h = i + 4;
                if h < buf.len() {
                    return Some(buf[h] & 0x1f);
                }
                return None;
            }
        }
        i += 1;
    }
    None
}

fn pack_i420_from_yuv_slices(
    y_src: &[u8],
    u_src: &[u8],
    v_src: &[u8],
    strides: (usize, usize, usize),
    width: usize,
    height: usize,
) -> Vec<u8> {
    let y_stride = strides.0;
    let u_stride = strides.1;
    let v_stride = strides.2;
    let uv_height = height / 2;
    let uv_width = width / 2;

    let mut out = Vec::with_capacity(width * height + 2 * uv_width * uv_height);

    for row in 0..height {
        let start = row * y_stride;
        out.extend_from_slice(&y_src[start..start + width]);
    }
    for row in 0..uv_height {
        let start = row * u_stride;
        out.extend_from_slice(&u_src[start..start + uv_width]);
    }
    for row in 0..uv_height {
        let start = row * v_stride;
        out.extend_from_slice(&v_src[start..start + uv_width]);
    }

    out
}

async fn log_transceivers(pc: &Arc<RTCPeerConnection>, label: &str) {
    let ts = pc.get_transceivers().await;
    println!("[receiver] transceivers {} count={}", label, ts.len());
    for (i, t) in ts.iter().enumerate() {
        println!(
            "[receiver] transceiver[{}] kind={} mid={:?} dir={} current={}",
            i,
            t.kind(),
            t.mid(),
            t.direction(),
            t.current_direction()
        );
    }
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
            for device in host
                .output_devices()
                .context("failed to enumerate output devices")?
            {
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
