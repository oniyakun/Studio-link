# Credits and Third-Party Attributions

This file tracks third-party projects, code snippets, and design assets used by this repository.

## Attribution policy
1. Any copied or adapted code must be recorded here in the same PR/change.
2. Record source URL, license, exact files used, and adaptation notes.
3. Keep original copyright/license notices when required.
4. If license is unclear, do not copy code until verified.

## Current status
No third-party code has been copied into this repository yet.
Third-party libraries are used as package/crate dependencies.

## Dependencies and references

### webrtc-rs/webrtc
- URL: https://github.com/webrtc-rs/webrtc
- Purpose: Rust WebRTC stack for Windows receiver
- License: MIT OR Apache-2.0
- Usage: In use as Rust crate dependency (`apps/windows-receiver-rust/Cargo.toml`)

### RustAudio/cpal
- URL: https://github.com/RustAudio/cpal
- Purpose: Cross-platform audio output abstraction (WASAPI backend on Windows)
- License: Apache-2.0
- Usage: In use as Rust crate dependency (`apps/windows-receiver-rust/Cargo.toml`)

### SpaceManiac/opus-rs
- URL: https://github.com/SpaceManiac/opus-rs
- Purpose: Opus decoder bindings for RTP payload decode in receiver
- License: BSD-3-Clause
- Usage: In use as Rust crate dependency (`apps/windows-receiver-rust/Cargo.toml`)

### websockets/ws
- URL: https://github.com/websockets/ws
- Purpose: Node.js signaling server WebSocket transport
- License: MIT
- Usage: In use as npm dependency (`apps/signaling-server/package.json`)

### webrtc/samples
- URL: https://github.com/webrtc/samples
- Purpose: Browser sender patterns (getUserMedia, peer negotiation, media constraints)
- License: BSD-3-Clause
- Planned usage: Architectural and API reference, optional adapted snippets

### suda/wireless-microphone
- URL: https://github.com/suda/wireless-microphone
- Purpose: Lightweight browser mic streaming flow and UX ideas
- License: MIT
- Planned usage: Signaling/flow inspiration and optional UI interaction ideas

### AlexxIT/go2rtc
- URL: https://github.com/AlexxIT/go2rtc
- Purpose: Optional fast validation path for browser ingest and relay behavior
- License: MIT
- Planned usage: Integration reference and debugging baseline

### VirtualDrivers/Virtual-Audio-Driver
- URL: https://github.com/VirtualDrivers/Virtual-Audio-Driver
- Purpose: Virtual audio device implementation reference (if needed)
- License: MIT
- Planned usage: Device behavior expectations and compatibility testing reference

## Attribution template
Use this block whenever code/assets are copied or adapted:

- Source: <url>
- Upstream project: <name>
- License: <spdx or text>
- Local files: <paths>
- Change type: copied | adapted | inspired
- Notes: <what was changed>
- Date: <YYYY-MM-DD>
