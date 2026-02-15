const statusEl = document.getElementById("status");
const signalingInput = document.getElementById("signalingUrl");
const roomInput = document.getElementById("roomId");
const micSelect = document.getElementById("micSelect");
const camSelect = document.getElementById("camSelect");
const refreshBtn = document.getElementById("refreshDevices");
const connectBtn = document.getElementById("connectBtn");

let ws;
let pc;
let localStream;

const ICE_SERVERS = [{ urls: "stun:stun.l.google.com:19302" }];

function log(line) {
  const now = new Date().toLocaleTimeString();
  statusEl.textContent = `[${now}] ${line}\n` + statusEl.textContent;
}

async function listDevices() {
  const devices = await navigator.mediaDevices.enumerateDevices();
  const mics = devices.filter((d) => d.kind === "audioinput");
  const cams = devices.filter((d) => d.kind === "videoinput");

  micSelect.innerHTML = mics
    .map((d, i) => `<option value="${d.deviceId}">${d.label || `Microphone ${i + 1}`}</option>`)
    .join("");

  camSelect.innerHTML = cams
    .map((d, i) => `<option value="${d.deviceId}">${d.label || `Camera ${i + 1}`}</option>`)
    .join("");
}

async function ensureMediaPermission() {
  const temp = await navigator.mediaDevices.getUserMedia({ audio: true, video: false });
  temp.getTracks().forEach((t) => t.stop());
}

function setupWebSocket() {
  return new Promise((resolve, reject) => {
    ws = new WebSocket(signalingInput.value.trim());

    ws.addEventListener("open", () => {
      log("signaling connected");
      ws.send(JSON.stringify({ type: "join", roomId: roomInput.value.trim(), role: "sender" }));
      resolve();
    });

    ws.addEventListener("error", (e) => reject(e));

    ws.addEventListener("message", async (event) => {
      const msg = JSON.parse(event.data);
      if (msg.type === "peer-ready") {
        await createOffer();
      } else if (msg.type === "answer") {
        await pc.setRemoteDescription(msg.data);
        log("remote answer set");
      } else if (msg.type === "ice" && msg.data) {
        await pc.addIceCandidate(msg.data);
      } else if (msg.type === "peer-left") {
        log("peer disconnected");
      } else if (msg.type === "error") {
        log(`server error: ${msg.reason}`);
      }
    });
  });
}

async function createPeerConnection() {
  pc = new RTCPeerConnection({ iceServers: ICE_SERVERS });

  pc.onconnectionstatechange = () => {
    log(`pc state: ${pc.connectionState}`);
  };

  pc.onicecandidate = (ev) => {
    if (!ev.candidate) return;
    ws.send(JSON.stringify({ type: "ice", data: ev.candidate }));
  };

  const micId = micSelect.value;
  localStream = await navigator.mediaDevices.getUserMedia({
    audio: {
      deviceId: micId ? { exact: micId } : undefined,
      channelCount: 1,
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    },
    video: false,
  });

  for (const track of localStream.getAudioTracks()) {
    pc.addTrack(track, localStream);
  }

  log("microphone track added");
}

async function createOffer() {
  if (!pc) return;
  if (pc.signalingState !== "stable") return;

  const offer = await pc.createOffer();
  await pc.setLocalDescription(offer);
  ws.send(JSON.stringify({ type: "offer", data: pc.localDescription }));
  log("offer sent");
}

function cleanup() {
  if (localStream) {
    localStream.getTracks().forEach((t) => t.stop());
  }
  if (pc) {
    pc.close();
  }
  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.close();
  }
}

refreshBtn.addEventListener("click", async () => {
  await ensureMediaPermission();
  await listDevices();
  log("device list refreshed");
});

connectBtn.addEventListener("click", async () => {
  cleanup();
  try {
    await ensureMediaPermission();
    await listDevices();
    await createPeerConnection();
    await setupWebSocket();
    log("waiting for host peer-ready");
  } catch (err) {
    log(`connect error: ${err.message || String(err)}`);
  }
});

(async () => {
  try {
    await ensureMediaPermission();
    await listDevices();
    log("ready");
  } catch {
    log("grant microphone permission first");
  }
})();
