# Studio Link

Browser microphone/camera relay to a Windows host application over WebRTC.

## Project layout
- `apps/sender-web`: Remote user web app (mic/camera capture, WebRTC sender)
- `apps/signaling-server`: WebSocket signaling service (SDP/ICE exchange only)
- `apps/windows-receiver-rust`: Windows Rust app (WebRTC receiver, audio/video output)
- `CREDITS.md`: Third-party code and license attributions

## Current status
Phase 2 in progress.

Implemented:
- WebSocket signaling server (`apps/signaling-server`)
- Browser sender page for microphone/camera capture and WebRTC publish (`apps/sender-web`)
- Rust Windows receiver (host role) that negotiates WebRTC
- Receiver audio path: RTP(Opus) -> PCM -> output device (WASAPI via `cpal`)
- Receiver video path: incoming H264 track decoded and sent to virtual camera backend
- Receiver virtual camera path: H264 video track -> OpenH264 decode -> `virtualcam` backend -> OBS Virtual Camera

Not implemented yet:
- Sender admission/auth and reconnect hardening

## Quick start

Prerequisites:
- Rust stable toolchain
- A browser with WebRTC support
- Virtual mic installed (like VB cable)
- OBS Virtual Camera installed (for virtual cam output)

## All-in-one executable (recommended)
The Rust receiver now includes embedded services in a single process:
- HTTPS sender web UI on `https://<host-ip>:5173`
- WSS signaling on `wss://<host-ip>:5173/ws`
- Local WS signaling bridge for host receiver on `ws://127.0.0.1:8787`

Build and run:

```powershell
cd apps/windows-receiver-rust
cargo build --release
.\target\release\windows-receiver-rust.exe
```

On first run, the app auto-creates `config.json` in the current directory.
Edit this file and restart the app to change behavior.

Open sender page on remote device:
- URL: `https://<host-ip>:5173`
- Signaling URL: `wss://<host-ip>:5173/ws`
- Click `Connect and Send`

Expected result:
- Receiver terminal prints `track received` and per-second RTP packet counts.
- If output device is your virtual mic playback endpoint, other apps can capture this audio via the corresponding virtual microphone input.
- When camera codec is H264 and OBS Virtual Camera is installed, receiver pushes decoded frames directly into OBS virtual cam backend.

## Configuration file (`config.json`)
Generated automatically on first run in the executable working directory.

```json
{
  "signaling_url": "ws://127.0.0.1:8787",
  "embedded_services": true,
  "virtual_mic_output_name": null,
  "virtual_cam_enabled": true,
  "virtual_cam_backend": "obs",
  "virtual_cam_width": null,
  "virtual_cam_height": null
}
```

Fields:
- `signaling_url`: receiver connects to this signaling endpoint.
- `embedded_services`: starts built-in HTTPS UI + WSS/WS signaling when `true`.
- `virtual_mic_output_name`: optional output device name substring.
- `virtual_cam_enabled`: enable/disable virtual cam output pipeline.
- `virtual_cam_backend`: virtualcam backend name (default `obs`).
- `virtual_cam_width` / `virtual_cam_height`: optional fixed virtual cam output size (even numbers).

## Direct OBS virtual camera injection (virtualcam crate)
The receiver now attempts direct virtual camera injection for H264 video tracks:

- Browser sender prefers H264 codec for video when available.
- Receiver decodes H264 and sends I420 frames to `virtualcam` backend `obs`.
- OBS Virtual Camera must be installed on Windows.

If direct injection fails or codec is not H264, virtual camera output is skipped.
