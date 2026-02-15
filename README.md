# web-rtc-mic

Browser microphone/camera relay to a Windows host application over WebRTC.

## Project layout
- `apps/sender-web`: Remote user web app (mic/camera capture, WebRTC sender)
- `apps/signaling-server`: WebSocket signaling service (SDP/ICE exchange only)
- `apps/windows-receiver-rust`: Windows Rust app (WebRTC receiver, audio/video output)
- `docs`: Architecture and roadmap
- `CREDITS.md`: Third-party code and license attributions

## Current status
Phase 2 in progress.

Implemented:
- WebSocket signaling server (`apps/signaling-server`)
- Browser sender page for microphone/camera capture and WebRTC publish (`apps/sender-web`)
- Rust Windows receiver (host role) that negotiates WebRTC
- Receiver audio path: RTP(Opus) -> PCM -> output device (WASAPI via `cpal`)
- Receiver video path: incoming video RTP track forwarded to local RTP sink and optional virtual camera output
- Receiver virtual camera path: H264 video track -> OpenH264 decode -> `virtualcam` backend -> OBS Virtual Camera

Not implemented yet:
- Sender admission/auth and reconnect hardening

## Quick start (audio + camera signaling + receive)

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
- Enable `Send Camera` to publish webcam track
- Click `Connect and Send`

Expected result:
- Receiver terminal prints `track received` and per-second RTP packet counts.
- If output device is your virtual mic playback endpoint, other apps can capture this audio via the corresponding virtual microphone input.
- When camera codec is H264 and OBS Virtual Camera is installed, receiver pushes decoded frames directly into OBS virtual cam backend.

## Environment variables (receiver)
- `SIGNALING_URL` (default `ws://127.0.0.1:8787`)
- `VIRTUAL_MIC_OUTPUT_NAME` (optional output device name substring; if omitted, uses default output device)
- `VIRTUAL_CAM_ENABLED` (default `true`)
- `VIRTUAL_CAM_BACKEND` (default `obs`)

## HTTPS / WSS mode (for browser mic permission)
By default, both Node services will auto-load:
- `certs/dev-cert.pem`
- `certs/dev-key.pem`

If you want to override cert paths, set env vars:

```powershell
$env:TLS_CERT_FILE="H:\\certs\\dev-cert.pem"
$env:TLS_KEY_FILE="H:\\certs\\dev-key.pem"
npm --workspace apps/signaling-server start
npm --workspace apps/sender-web start
```

Then open:
- Sender page: `https://<host-ip>:5173`
- Signaling URL in page: `wss://<host-ip>:8788`

Notes:
- Signaling server keeps `ws://<host-ip>:8787` active for local receiver.
- WSS endpoint uses `TLS_PORT` (default `8788`).

## Direct OBS virtual camera injection (virtualcam crate)
The receiver now attempts direct virtual camera injection for H264 video tracks:

- Browser sender prefers H264 codec for video when available.
- Receiver decodes H264 and sends I420 frames to `virtualcam` backend `obs`.
- OBS Virtual Camera must be installed on Windows.

If direct injection fails or codec is not H264, RTP forward/file recording still run as fallback.
