const statusEl = document.getElementById("status");
const signalingInput = document.getElementById("signalingUrl");
const micSelect = document.getElementById("micSelect");
const camSelect = document.getElementById("camSelect");
const enableVideoInput = document.getElementById("enableVideo");
const hidePreviewInput = document.getElementById("hidePreview");
const refreshBtn = document.getElementById("refreshDevices");
const connectBtn = document.getElementById("connectBtn");
const disconnectBtn = document.getElementById("disconnectBtn");
const localPreview = document.getElementById("localPreview");
const previewShell = document.querySelector(".preview-shell");
const connectionBadge = document.getElementById("connectionBadge");
const connectionBadgeText = document.getElementById("connectionBadgeText");
const mediaModeInputs = Array.from(document.querySelectorAll('input[name="mediaMode"]'));

let ws;
let pc;
let localStream;
let pendingRemoteIce = [];
let lastIceRestartAt = 0;
let statsTimer = null;
let mediaPermissionReady = false;

const ICE_SERVERS = [{ urls: "stun:stun.l.google.com:19302" }];
const FORCE_H264 = true;
const FIXED_ROOM_ID = "local-session";
const SETTINGS_KEY = "sender_web_settings_v1";

let preferredMicId = null;
let preferredCamId = null;

const defaultWsProtocol = location.protocol === "https:" ? "wss" : "ws";
const defaultWsPort = location.protocol === "https:" ? 8788 : 8787;
if (!signalingInput.value || signalingInput.value.includes("localhost")) {
  signalingInput.value = `${defaultWsProtocol}://${location.hostname}:${defaultWsPort}`;
}

function log(line) {
  const now = new Date().toLocaleTimeString();
  statusEl.textContent = `[${now}] ${line}\n` + statusEl.textContent;
}

function setConnectionBadge(state, text) {
  if (!connectionBadge || !connectionBadgeText) return;
  connectionBadge.dataset.state = state;
  connectionBadgeText.textContent = text;
}

function loadSettings() {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

function saveSettings() {
  const mode = getSelectedMediaMode().mode;
  const settings = {
    signalingUrl: signalingInput.value.trim(),
    micId: micSelect.value || "",
    camId: camSelect.value || "",
    mode,
    enableVideo: Boolean(enableVideoInput.checked),
    hidePreview: Boolean(hidePreviewInput?.checked),
  };
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(settings));
  } catch {
    // ignore quota/storage errors
  }
}

function applySavedSettings() {
  const settings = loadSettings();

  if (typeof settings.signalingUrl === "string" && settings.signalingUrl.trim()) {
    signalingInput.value = settings.signalingUrl.trim();
  }

  if (typeof settings.mode === "string") {
    const modeInput = mediaModeInputs.find((input) => input.value === settings.mode);
    if (modeInput) modeInput.checked = true;
  }

  if (typeof settings.enableVideo === "boolean") {
    enableVideoInput.checked = settings.enableVideo;
  }

  if (hidePreviewInput && typeof settings.hidePreview === "boolean") {
    hidePreviewInput.checked = settings.hidePreview;
  }

  if (typeof settings.micId === "string" && settings.micId) {
    preferredMicId = settings.micId;
  }
  if (typeof settings.camId === "string" && settings.camId) {
    preferredCamId = settings.camId;
  }
}

function restoreSelectValue(selectEl, preferred) {
  if (!preferred) return false;
  const hit = Array.from(selectEl.options).some((o) => o.value === preferred);
  if (hit) {
    selectEl.value = preferred;
    return true;
  }
  return false;
}

function updatePreviewVisibility() {
  if (!previewShell || !hidePreviewInput) return;
  previewShell.classList.toggle("preview-hidden", hidePreviewInput.checked);
}

function getSelectedMediaMode() {
  const checked = mediaModeInputs.find((input) => input.checked);
  const mode = checked?.value || "av";
  return {
    mode,
    wantAudio: mode === "av" || mode === "audio",
    wantVideo: mode === "av" || mode === "video",
  };
}

function getUserMediaCompat(constraints) {
  if (navigator.mediaDevices && navigator.mediaDevices.getUserMedia) {
    return navigator.mediaDevices.getUserMedia(constraints);
  }

  const legacy =
    navigator.getUserMedia ||
    navigator.webkitGetUserMedia ||
    navigator.mozGetUserMedia;

  if (!legacy) {
    throw new Error("getUserMedia API missing");
  }

  return new Promise((resolve, reject) => legacy.call(navigator, constraints, resolve, reject));
}

function logEnvironmentDiagnostics() {
  log(`origin=${location.origin}`);
  log(`protocol=${location.protocol} secureContext=${window.isSecureContext}`);
  log(`topFrame=${window.top === window.self}`);
  log(`mediaDevices=${Boolean(navigator.mediaDevices)} getUserMedia=${Boolean(navigator.mediaDevices?.getUserMedia)}`);
  log(`legacyGetUserMedia=${Boolean(navigator.getUserMedia || navigator.webkitGetUserMedia || navigator.mozGetUserMedia)}`);
}

async function listDevices() {
  if (!navigator.mediaDevices || !navigator.mediaDevices.enumerateDevices) {
    log("enumerateDevices unavailable");
    return;
  }
  const currentMic = micSelect.value;
  const currentCam = camSelect.value;
  const devices = await navigator.mediaDevices.enumerateDevices();
  const mics = devices.filter((d) => d.kind === "audioinput");
  const cams = devices.filter((d) => d.kind === "videoinput");

  micSelect.innerHTML = mics
    .map((d, i) => `<option value="${d.deviceId}">${d.label || `Microphone ${i + 1}`}</option>`)
    .join("");

  camSelect.innerHTML = cams
    .map((d, i) => `<option value="${d.deviceId}">${d.label || `Camera ${i + 1}`}</option>`)
    .join("");

  const micRestored = restoreSelectValue(micSelect, preferredMicId) || restoreSelectValue(micSelect, currentMic);
  const camRestored = restoreSelectValue(camSelect, preferredCamId) || restoreSelectValue(camSelect, currentCam);
  if (!micRestored) preferredMicId = null;
  if (!camRestored) preferredCamId = null;

  saveSettings();
}

async function ensureMediaPermission(wantAudio, wantVideo) {
  try {
    const constraints = {
      audio: wantAudio,
      video: wantVideo,
    };
    if (!constraints.audio && !constraints.video) {
      return;
    }
    const temp = await getUserMediaCompat(constraints);
    temp.getTracks().forEach((t) => t.stop());
  } catch (err) {
    const name = err?.name || "UnknownError";
    const message = err?.message || String(err);
    throw new Error(`${name}: ${message}`);
  }
}

async function preloadMediaPermissionsAndDevices() {
  try {
    await ensureMediaPermission(true, true);
    mediaPermissionReady = true;
    await listDevices();
    log("media permission granted; mic/camera list loaded");
  } catch (err) {
    mediaPermissionReady = false;
    log(`permission not granted yet: ${err.message || String(err)}`);
  }
}

function setupWebSocket() {
  return new Promise((resolve, reject) => {
    ws = new WebSocket(signalingInput.value.trim());

    ws.addEventListener("open", () => {
      log("signaling connected");
      setConnectionBadge("connecting", "SIGNALING UP");
      ws.send(JSON.stringify({ type: "join", roomId: FIXED_ROOM_ID, role: "sender" }));
      resolve();
    });

    ws.addEventListener("error", (e) => {
      setConnectionBadge("error", "SIGNAL ERROR");
      reject(e);
    });

    ws.addEventListener("close", () => {
      if (pc && pc.connectionState === "connected") return;
      setConnectionBadge("idle", "DISCONNECTED");
    });

    ws.addEventListener("message", async (event) => {
      const msg = JSON.parse(event.data);
      if (msg.type === "peer-ready") {
        setConnectionBadge("connecting", "NEGOTIATING");
        await createOffer(false);
      } else if (msg.type === "answer") {
        await pc.setRemoteDescription(msg.data);
        if (pendingRemoteIce.length > 0) {
          for (const candidate of pendingRemoteIce) {
            try {
              await pc.addIceCandidate(candidate);
            } catch (err) {
              log(`buffered ICE apply error: ${err.message || String(err)}`);
            }
          }
          pendingRemoteIce = [];
        }
        log("remote answer set");
      } else if (msg.type === "ice" && msg.data) {
        if (!pc.remoteDescription) {
          pendingRemoteIce.push(msg.data);
          return;
        }
        await pc.addIceCandidate(msg.data);
      } else if (msg.type === "peer-left") {
        log("peer disconnected");
        setConnectionBadge("warning", "PEER LEFT");
      } else if (msg.type === "error") {
        log(`server error: ${msg.reason}`);
        setConnectionBadge("error", "SERVER ERROR");
      }
    });
  });
}

async function createPeerConnection() {
  pc = new RTCPeerConnection({ iceServers: ICE_SERVERS });

  pc.onconnectionstatechange = () => {
    log(`pc state: ${pc.connectionState}`);
    if (pc.connectionState === "connected") {
      setConnectionBadge("streaming", "LIVE");
    } else if (pc.connectionState === "connecting") {
      setConnectionBadge("connecting", "CONNECTING");
    } else if (pc.connectionState === "disconnected") {
      setConnectionBadge("warning", "INTERRUPTED");
    } else if (pc.connectionState === "failed") {
      setConnectionBadge("error", "CONNECTION FAIL");
    } else if (pc.connectionState === "closed") {
      setConnectionBadge("idle", "DISCONNECTED");
    }
    if (pc.connectionState === "failed" || pc.connectionState === "disconnected") {
      maybeRestartIce();
    }
  };

  pc.oniceconnectionstatechange = () => {
    log(`ice state: ${pc.iceConnectionState}`);
    if (pc.iceConnectionState === "failed" || pc.iceConnectionState === "disconnected") {
      maybeRestartIce();
    }
  };

  pc.onicecandidate = (ev) => {
    if (!ev.candidate) return;
    ws.send(JSON.stringify({ type: "ice", data: ev.candidate }));
  };

  const micId = micSelect.value;
  const camId = camSelect.value;
  const mode = getSelectedMediaMode();
  const sendVideo = mode.wantVideo && enableVideoInput.checked;
  const constraints = {
    audio: mode.wantAudio
      ? {
          deviceId: micId ? { exact: micId } : undefined,
          channelCount: 1,
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        }
      : false,
    video: sendVideo
      ? {
          deviceId: camId ? { exact: camId } : undefined,
          width: { ideal: 1280 },
          height: { ideal: 720 },
          frameRate: { ideal: 30, max: 30 },
        }
      : false,
  };

  localStream = await getUserMediaCompat(constraints);

  localPreview.srcObject = localStream;

  for (const track of localStream.getAudioTracks()) {
    pc.addTrack(track, localStream);
  }
  for (const track of localStream.getVideoTracks()) {
    const sender = pc.addTrack(track, localStream);
    if (FORCE_H264) {
      try {
        const transceiver = pc.getTransceivers().find((t) => t.sender === sender);
        const videoCaps = RTCRtpSender.getCapabilities?.("video");
        const h264Codecs = (videoCaps?.codecs || []).filter(
          (c) => (c.mimeType || "").toLowerCase() === "video/h264"
        );
        if (transceiver && videoCaps?.codecs?.length > 0 && transceiver.setCodecPreferences) {
          const others = videoCaps.codecs.filter(
            (c) => (c.mimeType || "").toLowerCase() !== "video/h264"
          );
          const preferred = h264Codecs.length > 0 ? [...h264Codecs, ...others] : videoCaps.codecs;
          transceiver.setCodecPreferences(preferred);
          log(
            `video codec preference set: H264-first=${h264Codecs.length > 0} total=${preferred.length}`
          );
        }
      } catch (err) {
        log(`video codec preference skipped: ${err.message || String(err)}`);
      }
    }
  }

  log(`tracks added audio=${localStream.getAudioTracks().length} video=${localStream.getVideoTracks().length}`);
  if (mode.wantVideo && sendVideo && localStream.getVideoTracks().length === 0) {
    log("warning: Send Camera is enabled but no video track was acquired");
  }
  if (mode.wantAudio && localStream.getAudioTracks().length === 0) {
    log("warning: no audio track acquired");
  }
  if (localStream.getAudioTracks().length === 0 && localStream.getVideoTracks().length === 0) {
    throw new Error("no media track acquired for selected mode");
  }
}

async function createOffer(iceRestart = false) {
  if (!pc) return;
  if (pc.signalingState !== "stable") return;

  const offer = await pc.createOffer({ iceRestart });
  const normalizedOffer = {
    type: offer.type,
    sdp: normalizeH264ForDecoder(offer.sdp || ""),
  };
  await pc.setLocalDescription(normalizedOffer);
  ws.send(JSON.stringify({ type: "offer", data: pc.localDescription }));
  log(iceRestart ? "ice-restart offer sent" : "offer sent");
}

async function maybeRestartIce() {
  const now = Date.now();
  if (now - lastIceRestartAt < 5000) return;
  if (!pc || !ws || ws.readyState !== WebSocket.OPEN) return;
  if (pc.signalingState !== "stable") return;
  lastIceRestartAt = now;
  try {
    await createOffer(true);
  } catch (err) {
    log(`ice restart error: ${err.message || String(err)}`);
  }
}

function normalizeH264ForDecoder(sdp) {
  if (!FORCE_H264) return sdp;
  if (!sdp || !sdp.includes("H264/90000")) return sdp;

  const lines = sdp.split("\r\n");
  const out = [];
  for (const line of lines) {
    if (line.startsWith("a=fmtp:") && line.includes("profile-level-id=")) {
      let next = line.replace(/profile-level-id=[0-9A-Fa-f]+/g, "profile-level-id=42e01f");
      if (!/packetization-mode=\d/.test(next)) {
        next += ";packetization-mode=1";
      }
      out.push(next);
      continue;
    }
    out.push(line);
  }
  return out.join("\r\n");
}

function startSenderStats() {
  if (!pc) return;
  if (statsTimer) clearInterval(statsTimer);
  statsTimer = setInterval(async () => {
    if (!pc || pc.connectionState === "closed") return;
    try {
      const stats = await pc.getStats();
      let audioLine = null;
      let videoLine = null;
      stats.forEach((r) => {
        if (r.type === "outbound-rtp" && !r.isRemote) {
          if (r.kind === "audio") {
            audioLine = `audio_out packets=${r.packetsSent || 0} bytes=${r.bytesSent || 0}`;
          } else if (r.kind === "video") {
            videoLine = `video_out packets=${r.packetsSent || 0} bytes=${r.bytesSent || 0} frames=${r.framesSent || 0}`;
          }
        }
      });
      if (audioLine || videoLine) {
        log([audioLine, videoLine].filter(Boolean).join(" | "));
      }
    } catch (err) {
      log(`stats error: ${err.message || String(err)}`);
    }
  }, 3000);
}

function cleanup() {
  pendingRemoteIce = [];
  if (statsTimer) {
    clearInterval(statsTimer);
    statsTimer = null;
  }
  if (localStream) {
    localStream.getTracks().forEach((t) => t.stop());
  }
  localStream = null;
  localPreview.srcObject = null;
  if (pc) {
    pc.close();
  }
  pc = null;
  if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) {
    ws.close();
  }
  ws = null;
  connectBtn.disabled = false;
  disconnectBtn.disabled = true;
  setConnectionBadge("idle", "DISCONNECTED");
}

function updateModeDependentUi() {
  const mode = getSelectedMediaMode();
  micSelect.disabled = !mode.wantAudio;
  camSelect.disabled = !mode.wantVideo;
  enableVideoInput.disabled = !mode.wantVideo;
  saveSettings();
}

refreshBtn.addEventListener("click", async () => {
  try {
    await preloadMediaPermissionsAndDevices();
    if (mediaPermissionReady) {
      log("device list refreshed");
    }
  } catch (err) {
    log(`permission error: ${err.message || String(err)}`);
  }
});

connectBtn.addEventListener("click", async () => {
  cleanup();
  try {
    setConnectionBadge("connecting", "CONNECTING");
    if (!mediaPermissionReady) {
      await preloadMediaPermissionsAndDevices();
    }
    const mode = getSelectedMediaMode();
    await ensureMediaPermission(mode.wantAudio, mode.wantVideo && enableVideoInput.checked);
    mediaPermissionReady = true;
    await listDevices();
    updateModeDependentUi();
    await createPeerConnection();
    await setupWebSocket();
    startSenderStats();
    connectBtn.disabled = true;
    disconnectBtn.disabled = false;
    log("waiting for host peer-ready");
    log(`mode selected: ${mode.mode}`);
  } catch (err) {
    log(`connect error: ${err.message || String(err)}`);
    cleanup();
  }
});

disconnectBtn.addEventListener("click", () => {
  cleanup();
  log("disconnected by user");
  setConnectionBadge("idle", "DISCONNECTED");
});

for (const input of mediaModeInputs) {
  input.addEventListener("change", () => {
    updateModeDependentUi();
    saveSettings();
  });
}

signalingInput.addEventListener("change", saveSettings);
micSelect.addEventListener("change", saveSettings);
camSelect.addEventListener("change", saveSettings);
enableVideoInput.addEventListener("change", () => {
  saveSettings();
  updateModeDependentUi();
});
if (hidePreviewInput) {
  hidePreviewInput.addEventListener("change", () => {
    updatePreviewVisibility();
    saveSettings();
  });
}

(async () => {
  applySavedSettings();
  updatePreviewVisibility();
  logEnvironmentDiagnostics();
  await preloadMediaPermissionsAndDevices();
  if (!mediaPermissionReady) {
    await listDevices();
  }
  updateModeDependentUi();
  saveSettings();
  disconnectBtn.disabled = true;
  setConnectionBadge("idle", "READY");
  log("ready (permissions requested before connect)");
})();
