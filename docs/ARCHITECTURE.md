# Architecture (A/V Relay)

## Goal
Relay remote browser microphone and camera streams to a Windows host app via WebRTC.

## End-to-end flow
1. Host user starts Windows receiver app and creates a room/session.
2. Remote user opens sender web page and joins room.
3. Sender web captures `getUserMedia({ audio: true, video: true })`.
4. Signaling server exchanges SDP/ICE between web sender and Windows receiver.
5. Receiver gets two tracks:
- Audio track -> PCM pipeline -> existing virtual microphone device (WASAPI)
- Video track -> frame pipeline -> preview window and optional virtual camera sink
6. Other apps (game/chat tools) use host virtual mic (and virtual cam if available).

## Reuse-first component choices
- WebRTC in Rust receiver: `webrtc-rs/webrtc`
- Browser implementation references: `webrtc/samples`
- Signaling pattern references: `node-webrtc-examples`, `wireless-microphone`
- Optional media gateway alternative for fast validation: `go2rtc`

## Receiver module split (Rust)
- `signal_client`: WebSocket signaling, reconnect and heartbeat
- `rtc_peer`: RTCPeerConnection lifecycle, transceivers, ICE
- `audio_pipeline`: jitter buffer, sample-rate/channel conversion, WASAPI output
- `video_pipeline`: frame decode/convert, preview, optional virtual camera output
- `device_output`: host virtual mic and optional virtual camera adapters
- `session`: room auth, sender admission policy, state machine

## Runtime constraints
- Audio canonical format: 48kHz mono 16-bit PCM
- Video target format: 720p@30 or 1080p@30 depending CPU budget
- Prefer single active sender per room (simpler routing and UX)

## Security baseline
- Join token or short PIN for room binding
- Explicit sender approval on host app
- DTLS-SRTP provided by WebRTC stack

## Observability
- Receiver metrics: RTT, packet loss, jitter, decode latency, audio underruns
- User-facing diagnostics: connected/disconnected, current devices, peak audio level
