import { WebSocketServer } from "ws";
import { randomUUID } from "node:crypto";

const PORT = Number(process.env.PORT || 8787);

const wss = new WebSocketServer({ port: PORT });

/** @type {Map<string, {host: import('ws').WebSocket | null, sender: import('ws').WebSocket | null}>} */
const rooms = new Map();

function send(ws, payload) {
  if (ws.readyState === ws.OPEN) {
    ws.send(JSON.stringify(payload));
  }
}

function peerOf(room, role) {
  if (!room) return null;
  return role === "host" ? room.sender : room.host;
}

function ensureRoom(roomId) {
  if (!rooms.has(roomId)) {
    rooms.set(roomId, { host: null, sender: null });
  }
  return rooms.get(roomId);
}

function roleIsValid(role) {
  return role === "host" || role === "sender";
}

wss.on("connection", (ws) => {
  const clientId = randomUUID();
  /** @type {{roomId: string | null, role: "host" | "sender" | null}} */
  const state = { roomId: null, role: null };

  send(ws, { type: "connected", clientId });

  ws.on("message", (raw) => {
    let msg;
    try {
      msg = JSON.parse(raw.toString());
    } catch {
      send(ws, { type: "error", reason: "Invalid JSON" });
      return;
    }

    if (msg.type === "join") {
      const roomId = String(msg.roomId || "").trim();
      const role = String(msg.role || "").trim();
      if (!roomId || !roleIsValid(role)) {
        send(ws, { type: "error", reason: "join requires roomId and role(host|sender)" });
        return;
      }

      const room = ensureRoom(roomId);
      if (room[role] && room[role] !== ws) {
        send(ws, { type: "error", reason: `${role} already joined room` });
        return;
      }

      if (state.roomId && state.role) {
        const prev = rooms.get(state.roomId);
        if (prev && prev[state.role] === ws) {
          prev[state.role] = null;
        }
      }

      state.roomId = roomId;
      state.role = role;
      room[role] = ws;

      send(ws, { type: "joined", roomId, role });

      const peer = peerOf(room, role);
      if (peer) {
        send(peer, { type: "peer-ready", roomId });
        send(ws, { type: "peer-ready", roomId });
      }
      return;
    }

    if (!state.roomId || !state.role) {
      send(ws, { type: "error", reason: "join first" });
      return;
    }

    if (msg.type === "offer" || msg.type === "answer" || msg.type === "ice") {
      const room = rooms.get(state.roomId);
      const peer = peerOf(room, state.role);
      if (!peer) {
        send(ws, { type: "error", reason: "peer not connected" });
        return;
      }

      send(peer, {
        type: msg.type,
        roomId: state.roomId,
        from: state.role,
        data: msg.data,
      });
      return;
    }

    send(ws, { type: "error", reason: `Unknown type: ${msg.type}` });
  });

  ws.on("close", () => {
    if (!state.roomId || !state.role) return;

    const room = rooms.get(state.roomId);
    if (!room) return;

    if (room[state.role] === ws) {
      room[state.role] = null;
    }

    const peer = peerOf(room, state.role);
    if (peer) {
      send(peer, { type: "peer-left", roomId: state.roomId, role: state.role });
    }

    if (!room.host && !room.sender) {
      rooms.delete(state.roomId);
    }
  });
});

console.log(`[signaling] ws://0.0.0.0:${PORT}`);
