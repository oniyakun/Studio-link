# Roadmap

## Phase 1 - Audio-only MVP
1. Build signaling server (room join, SDP/ICE relay).
2. Build sender web page: choose mic and push single audio track.
3. Build Rust receiver: receive audio track and print level meter.
4. Output received audio to existing virtual microphone via WASAPI.
5. Validate with Windows recorder and one game/chat app.

## Phase 2 - Add camera passthrough
1. Extend sender page to capture camera and send video track.
2. Extend receiver to handle video track and render local preview.
3. If virtual camera exists: write frames to virtual camera input.
4. Validate in conferencing app/game that virtual camera is visible.

## Phase 3 - Reliability and latency
1. Reconnect logic for signaling and peer connection.
2. Adaptive jitter buffering and backpressure for audio pipeline.
3. Device hot-swap handling (sender mic/cam switch, host device switch).
4. Operational logs and simple metrics endpoint.

## Phase 4 - Productization
1. Host UI: room code, sender approval, live stats.
2. Access control: token/PIN and single-session lock.
3. Packaging for Windows distribution.

## Definition of done
- Audio path: remote speech is heard from host virtual microphone in third-party apps.
- Video path: remote camera feed appears in host virtual camera consumers (if virtual cam available).
- Recovery: temporary network drop reconnects automatically without app restart.
