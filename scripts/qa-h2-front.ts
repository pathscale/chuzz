// QA fixture front: TLS with ALPN h2 only, proxying to the plain fixture.
// The CA and leaf are ECDSA P-256 and live only in --dir, a temp directory.
// Chuzz trusts the CA through SSL_CERT_FILE=<dir>/ca.pem for that process
// only; nothing is added to any system or user trust store.
import { createSecureServer, constants } from "node:http2";
import { existsSync } from "node:fs";
import { installCaps } from "./app-caps";
const caps = installCaps("chuzz-h2-front");

function arg(name: string): string {
  const index = Bun.argv.indexOf(`--${name}`);
  const value = index > 0 ? Bun.argv[index + 1] : undefined;
  if (!value) {
    console.error("usage: qa-h2-front.ts --dir TEMP_DIR --listen PORT --upstream PORT");
    process.exit(2);
  }
  return value;
}

const dir = arg("dir");
const listen = Number(arg("listen"));
const upstream = Number(arg("upstream"));

function openssl(...args: string[]) {
  caps.turn("certificate command");
  caps.beforeChild();
  const run = Bun.spawnSync(["openssl", ...args], { cwd: dir, stderr: "pipe" });
  if (run.exitCode !== 0) {
    console.error(`openssl ${args[0]} failed: ${run.stderr.toString()}`);
    process.exit(1);
  }
}

if (!existsSync(`${dir}/leaf.pem`)) {
  openssl("ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", "ca.key");
  openssl("req", "-x509", "-new", "-key", "ca.key", "-sha256", "-days", "2",
    "-subj", "/CN=chuzz QA test CA", "-addext", "basicConstraints=critical,CA:TRUE",
    "-addext", "keyUsage=critical,keyCertSign,cRLSign", "-out", "ca.pem");
  openssl("ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", "leaf.key");
  openssl("req", "-new", "-key", "leaf.key", "-subj", "/CN=127.0.0.1", "-out", "leaf.csr");
  await Bun.write(`${dir}/leaf.ext`,
    "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1,DNS:localhost\n");
  openssl("x509", "-req", "-in", "leaf.csr", "-CA", "ca.pem", "-CAkey", "ca.key",
    "-CAcreateserial", "-sha256", "-days", "2", "-extfile", "leaf.ext", "-out", "leaf.pem");
}

const hopByHop = new Set(["connection", "keep-alive", "transfer-encoding", "upgrade", "proxy-connection"]);

const server = createSecureServer({
  key: await caps.text(`${dir}/leaf.key`),
  cert: await caps.text(`${dir}/leaf.pem`),
  ALPNProtocols: ["h2"],
  allowHTTP1: false,
});

let streams = 0;
server.on("session", () => console.log("h2 session"));
server.on("stream", async (stream, headers) => {
  caps.turn("h2 stream");
  streams += 1;
  const method = String(headers[constants.HTTP2_HEADER_METHOD]);
  const path = String(headers[constants.HTTP2_HEADER_PATH]);
  const forward = new Headers();
  for (const [name, value] of Object.entries(headers)) {
    if (name.startsWith(":") || hopByHop.has(name) || value === undefined) continue;
    forward.set(name, Array.isArray(value) ? value.join(", ") : String(value));
  }
  forward.set("host", `127.0.0.1:${listen}`);
  const chunks: Buffer[] = [];
  let requestBytes = 0;
  for await (const chunk of stream) {
    requestBytes += (chunk as Buffer).byteLength;
    caps.check("input-bytes", requestBytes);
    caps.check("cache-entries", chunks.length + 1);
    chunks.push(chunk as Buffer);
  }
  caps.reserve(requestBytes);
  try {
    const response = await fetch(`http://127.0.0.1:${upstream}${path}`, {
      method,
      headers: forward,
      body: method === "GET" || method === "HEAD" ? undefined : Buffer.concat(chunks),
      redirect: "manual",
    });
    const out: Record<string, string | number> = { ":status": response.status };
    response.headers.forEach((value, name) => {
      if (!hopByHop.has(name) && name !== "content-length") out[name] = value;
    });
    const bytes = response.body ? await caps.bytes(response.body, "h2 upstream") : new Uint8Array();
    const body = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    out["content-length"] = body.length;
    if (stream.destroyed) return;
    stream.respond(out);
    stream.end(method === "HEAD" ? undefined : body);
  } catch (error) {
    if (stream.destroyed) return;
    stream.respond({ ":status": 502, "content-type": "text/plain" });
    stream.end(`fixture upstream failed: ${error}`);
  }
});
server.listen(listen, "127.0.0.1", () => console.log(`h2 front https://127.0.0.1:${listen}/ -> ${upstream}`));
process.on("SIGTERM", () => { console.log(`h2 streams: ${streams}`); process.exit(0); });
