import fs from "node:fs";
import http from "node:http";
import https from "node:https";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const publicDir = path.join(__dirname, "..", "public");
const PORT = Number(process.env.PORT || 5173);
const defaultCert = path.resolve(__dirname, "..", "..", "..", "certs", "dev-cert.pem");
const defaultKey = path.resolve(__dirname, "..", "..", "..", "certs", "dev-key.pem");
const TLS_CERT_FILE = process.env.TLS_CERT_FILE || defaultCert;
const TLS_KEY_FILE = process.env.TLS_KEY_FILE || defaultKey;
const useTls = fs.existsSync(TLS_CERT_FILE) && fs.existsSync(TLS_KEY_FILE);

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
};

const requestHandler = async (req, res) => {
  try {
    const reqPath = req.url === "/" ? "/index.html" : req.url;
    const absPath = path.normalize(path.join(publicDir, reqPath));

    if (!absPath.startsWith(publicDir)) {
      res.writeHead(403);
      res.end("Forbidden");
      return;
    }

    const data = await readFile(absPath);
    const ext = path.extname(absPath);
    res.writeHead(200, { "Content-Type": MIME[ext] || "application/octet-stream" });
    res.end(data);
  } catch {
    res.writeHead(404);
    res.end("Not found");
  }
};

const server = useTls
  ? https.createServer(
      {
        cert: fs.readFileSync(TLS_CERT_FILE),
        key: fs.readFileSync(TLS_KEY_FILE),
      },
      requestHandler
    )
  : http.createServer(requestHandler);

server.listen(PORT, () => {
  if (!useTls) {
    console.warn("[sender-web] TLS cert/key not found, fallback to HTTP");
  }
  console.log(`[sender-web] ${useTls ? "https" : "http"}://0.0.0.0:${PORT}`);
});
