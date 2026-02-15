# web-rtc-mic

Browser microphone/camera relay to a Windows host application over WebRTC.

## Project layout
- `apps/sender-web`: Remote user web app (mic/camera capture, WebRTC sender)
- `apps/signaling-server`: WebSocket signaling service (SDP/ICE exchange only)
- `apps/windows-receiver-rust`: Windows Rust app (WebRTC receiver, audio/video output)
- `docs`: Architecture and roadmap
- `CREDITS.md`: Third-party code and license attributions

## Current status
Phase 1 in progress.

Implemented:
- WebSocket signaling server (`apps/signaling-server`)
- Browser sender page for microphone capture and WebRTC publish (`apps/sender-web`)
- Rust Windows receiver (host role) that negotiates WebRTC and logs inbound RTP packet rate (`apps/windows-receiver-rust`)

Not implemented yet:
- Writing received audio into virtual microphone device (WASAPI output)
- Camera passthrough

## Quick start (audio signaling + receive)

Prerequisites:
- Node.js 20+
- Rust stable toolchain
- A browser with WebRTC support

Install JS dependencies:

```bash
npm install
```

Start signaling server:

```bash
npm --workspace apps/signaling-server start
```

Start sender web:

```bash
npm --workspace apps/sender-web start
```

Run Rust receiver (Windows host):

```bash
cd apps/windows-receiver-rust
cargo run
```

Open sender page on remote device:
- URL: `http://<host-ip>:5173`
- Signaling URL: `ws://<host-ip>:8787`
- Room ID: same value as receiver (default `demo-room`)
- Click `Connect and Send`

Expected result:
- Receiver terminal prints `track received` and per-second RTP packet counts.

## Environment variables (receiver)
- `SIGNALING_URL` (default `ws://127.0.0.1:8787`)
- `ROOM_ID` (default `demo-room`)
