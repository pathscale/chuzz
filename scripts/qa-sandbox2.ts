import { cp, mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createServer, type Server } from "node:https";
import { connect } from "node:tls";
import { installCaps } from "./app-caps";
import { verifySignature } from "./verify-sandbox2-signature";

const caps = installCaps("chuzz-sandbox2");
const repository = resolve(import.meta.dir, "..");
const fixture = join(repository, "apps/chuzz/src/sandbox/qa");
const bundleArgument = Bun.argv.indexOf("--bundle");
if (bundleArgument >= 0 && !Bun.argv[bundleArgument + 1]) throw new Error("--bundle needs a path");
const bundle = (bundleArgument >= 0 ? resolve(Bun.argv[bundleArgument + 1]) : process.env.QA_APP_BUNDLE) || join(repository, "target/release/bundle/macos/Chuzz.app");
const shipped = process.env.QA_HOST_BIN || join(bundle, "Contents/MacOS/chuzz-headless");
const gui = process.env.QA_GUI_BIN || join(bundle, "Contents/MacOS/chuzz-gui");
const harness = process.env.PS_QA_BIN || "ps-qa";
const children = new Set<Bun.Subprocess>();
const names = ["passwd-denied", "home-denied", "write-denied", "spawn-denied", "cache-readable", "core-limit-zero", "heap-reports-disabled"];

async function run(argv: string[], env: Record<string, string | undefined> = process.env) {
  caps.turn(argv[0]); caps.beforeChild();
  const child = Bun.spawn(argv, { env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  caps.watch(child); children.add(child);
  const decoded = (stream: ReadableStream<Uint8Array>) => caps.bytes(stream, "sandbox2 child output", 16 * 1024 * 1024).then(bytes => new TextDecoder().decode(bytes));
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const result = Promise.all([child.exited, decoded(child.stdout), decoded(child.stderr)]);
    const [code, stdout, stderr] = await Promise.race([result, new Promise<never>((_, reject) => {
      timer = setTimeout(() => { child.kill("SIGKILL"); reject(new Error(`sandbox2-deadline: ${argv[0]}`)); }, 90000);
    })]);
    return { code, stdout, stderr };
  } finally { clearTimeout(timer); children.delete(child); }
}

async function requireCommand(argv: string[]) {
  const result = await run(argv);
  if (result.code !== 0) throw new Error(`${argv[0]} exited ${result.code}: ${result.stderr}`);
  return result;
}

function cells(line: string): string[] {
  const cells: string[] = []; let start = 0, quoted = false, escaped = false;
  for (let index = 0; index < line.length; index++) {
    const character = line[index];
    if (escaped) { escaped = false; continue; }
    if (quoted && character === "\\") { escaped = true; continue; }
    if (character === '"') { quoted = !quoted; }
    if (character === "," && !quoted) { cells.push(line.slice(start, index).trim()); start = index + 1; }
  }
  if (quoted || escaped) throw new Error("sandbox2-report: malformed TOON quoting");
  cells.push(line.slice(start).trim());
  return cells.map(cell => cell.startsWith('"') ? String(JSON.parse(cell)) : cell);
}

function rows(stdout: string): Record<string, string>[] {
  const headers = [...stdout.matchAll(/^checks\[(\d+)\]\{([^}]+)\}:\r?$/gm)];
  if (headers.length !== 1) throw new Error("sandbox2-report: one ps-qa checks table required");
  const header = headers[0], count = Number(header[1]), columns = header[2].split(",").map(value => value.trim());
  if (!["id", "verdict", "error"].every(column => columns.includes(column))) throw new Error("sandbox2-report: incomplete columns");
  const lines = stdout.slice(header.index! + header[0].length).split(/\r?\n/).slice(1, count + 1);
  if (lines.length !== count) throw new Error("sandbox2-report: truncated checks table");
  return lines.map(line => {
    const values = cells(line.trim());
    if (values.length !== columns.length) throw new Error("sandbox2-report: wrong row width");
    return Object.fromEntries(columns.map((column, index) => [column, values[index]]));
  });
}

function judge(result: { code: number; stdout: string; stderr: string }, ids: string[], failure?: string) {
  const checks = rows(result.stdout);
  if (checks.length !== ids.length || new Set(checks.map(row => row.id)).size !== ids.length) throw new Error("sandbox2-report: missing or duplicate check ids");
  for (const id of ids) {
    const row = checks.find(row => row.id === id);
    const expected = failure === id ? "fail" : "pass";
    if (!row || row.verdict.toLowerCase() !== expected) throw new Error(`${id}: expected ${expected}: ${result.stdout}\n${result.stderr}`);
  }
  if (result.code !== (failure ? 1 : 0)) throw new Error(`sandbox2-report: unexpected exit ${result.code}`);
  console.log(failure ? `planted defect failed by name: ${failure}` : `functional checks passed: ${ids.join(", ")}`);
}

function startup(result: { code: number; stdout: string; stderr: string }, failure?: string) {
  const found = [...result.stdout.matchAll(/^sandbox2: (\S+) (PASS|FAIL)$/gm)];
  if (found.length !== names.length) throw new Error(`sandbox2-startup: missing real probe results: ${result.stderr}`);
  for (const name of names) {
    const matches = found.filter(row => row[1] === name);
    if (matches.length !== 1 || matches[0][2] !== (failure === name ? "FAIL" : "PASS")) throw new Error(`sandbox2-startup: ${name} verdict mismatch`);
  }
  if (result.code !== (failure ? 1 : 0)) throw new Error(`sandbox2-startup: exit ${result.code}`);
}

const work = await mkdtemp(join(tmpdir(), "chuzz-sandbox2-"));
const servers: Server[] = [];
const sockets = new Set<import("node:net").Socket>();
let sequence = 0;
async function environment(extra: Record<string, string | undefined> = {}) {
  const cache = join(work, `cache-${sequence++}`);
  await mkdir(cache, { mode: 0o700 });
  return { ...process.env, CHUZZ_CACHE_DIR: cache, CHUZZ_PROFILE_DIR: join(cache, "profile"), CHUZZ_HEADLESS_MODE: "qa", PS_QA_HEADLESS: "true", SANDBOX2_DEFECT: undefined, SANDBOX2_TLS_DEFECT: undefined, ...extra };
}

async function qa(host: string, page: string, checks: string, env: Record<string, string | undefined>) {
  return run([harness, "qa-hosted", "--host", host, "--page", page, "--app", join(fixture, "ps-qa.ron"), "--checks", join(fixture, "checks", checks)], env);
}

async function sign(path: string, entitlements: string) {
  await requireCommand(["/usr/bin/codesign", "--force", "--sign", "-", "--options", "runtime", "--entitlements", entitlements, path]);
}

async function tlsServer(cert: string, key: string, ca: string, version: "TLSv1.2" | "TLSv1.3", cipher: string) {
  const server = createServer({ cert, key, minVersion: version, maxVersion: version, ciphers: cipher, ALPNProtocols: ["http/1.1"] }, (_request, response) => {
    response.writeHead(200, { "content-type": "text/html", "cache-control": "no-store" });
    response.end('<!doctype html><html><body style="background:white;color:black;font:24px sans-serif"><h1 id="chacha-loaded">CHACHA LOADED</h1></body></html>');
  });
  server.on("connection", socket => { sockets.add(socket); socket.once("close", () => sockets.delete(socket)); });
  server.on("tlsClientError", () => {});
  servers.push(server);
  await new Promise<void>((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("TLS fixture did not bind TCP");
  // Independently confirm that the fixture can authenticate and negotiates
  // its declared single suite before asking chuzz to refuse or load it.
  await new Promise<void>((resolve, reject) => {
    const socket = connect({ host: "127.0.0.1", port: address.port, servername: "localhost", ca, minVersion: version, maxVersion: version, ciphers: cipher, ALPNProtocols: ["http/1.1"] });
    socket.once("error", reject);
    socket.once("secureConnect", () => {
      const valid = socket.getProtocol() === version && socket.getCipher().name === cipher;
      socket.destroy();
      if (valid) resolve(); else reject(new Error(`TLS fixture did not negotiate ${version} ${cipher}`));
    });
  });
  return `https://127.0.0.1:${address.port}/`;
}

try {
  if (process.platform !== "darwin") throw new Error("sandbox2 functional checks require macOS");
  for (const binary of [shipped, gui]) {
    await verifySignature(binary);
    startup(await run([binary, "--sandbox-probe"], await environment()));
  }
  if (!Bun.argv.includes("--startup-only")) {
    const input = process.env.QA_HOST_WITNESS_BIN;
    if (!input) throw new Error("QA_HOST_WITNESS_BIN must name an already-linked sandbox2-witness headless binary");
    const witness = join(work, "chuzz-witness");
    await cp(input, witness);
    const entitlements = join(repository, "apps/chuzz/entitlements.plist");
    await sign(witness, entitlements);
    await verifySignature(witness);
    judge(await qa(shipped, "chuzz:sandbox-probe", "sandbox.ron", await environment()), names.map(name => `sandbox2-${name}`));
    for (const name of names) {
      const env = await environment({ SANDBOX2_DEFECT: name });
      startup(await run([witness, "--sandbox-probe"], env), name);
      judge(await qa(witness, "chuzz:sandbox-probe", "sandbox.ron", await environment({ SANDBOX2_DEFECT: name })), names.map(name => `sandbox2-${name}`), `sandbox2-${name}`);
    }
    // Exercise the real signing policy through an intentionally extra entitlement.
    const wrong = join(work, "wrong-entitlements.plist"), wronglySigned = join(work, "wrongly-signed");
    const declared = await Bun.file(entitlements).text();
    await Bun.write(wrong, declared.replace("</dict>", "<key>com.apple.security.cs.disable-library-validation</key><true/></dict>"));
    await cp(input, wronglySigned); await sign(wronglySigned, wrong);
    let rejected = false;
    try { await verifySignature(wronglySigned); } catch (error) { rejected = String(error).includes("entitlements-exact"); }
    if (!rejected) throw new Error("entitlements-exact: planted extra entitlement was accepted");
    console.log("planted defect failed by name: entitlements-exact");
    const tls = join(work, "tls"); await mkdir(tls);
    const config = join(tls, "ca.conf"), caFile = join(tls, "ca.pem"), caKey = join(tls, "ca.key"), leafKey = join(tls, "leaf.key"), leafCsr = join(tls, "leaf.csr"), leafFile = join(tls, "leaf.pem"), leafExt = join(tls, "leaf.ext");
    await Bun.write(config, "[req]\ndistinguished_name=dn\nx509_extensions=ca\nprompt=no\n[dn]\nCN=chuzz local QA CA\n[ca]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\n");
    await requireCommand(["openssl", "ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", caKey]);
    await requireCommand(["openssl", "req", "-x509", "-new", "-key", caKey, "-sha256", "-days", "2", "-config", config, "-out", caFile]);
    await requireCommand(["openssl", "ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", leafKey]);
    await requireCommand(["openssl", "req", "-new", "-key", leafKey, "-subj", "/CN=localhost", "-out", leafCsr]);
    await Bun.write(leafExt, "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1,DNS:localhost\n");
    await requireCommand(["openssl", "x509", "-req", "-in", leafCsr, "-CA", caFile, "-CAkey", caKey, "-CAcreateserial", "-sha256", "-days", "2", "-extfile", leafExt, "-out", leafFile]);
    const cert = await Bun.file(leafFile).text(), key = await Bun.file(leafKey).text(), ca = await Bun.file(caFile).text();
    for (const version of ["TLSv1.2", "TLSv1.3"] as const) {
      const chacha = await tlsServer(cert, key, ca, version, version === "TLSv1.2" ? "ECDHE-ECDSA-CHACHA20-POLY1305" : "TLS_CHACHA20_POLY1305_SHA256");
      const aes = await tlsServer(cert, key, ca, version, version === "TLSv1.2" ? "ECDHE-ECDSA-AES128-GCM-SHA256" : "TLS_AES_128_GCM_SHA256");
      const trust = { SSL_CERT_FILE: caFile, SANDBOX2_TLS_CA_FILE: caFile };
      judge(await qa(shipped, chacha, "chacha.ron", await environment(trust)), ["tls-chacha-load"]);
      judge(await qa(shipped, aes, "required.ron", await environment(trust)), ["tls-chacha-required"]);
      judge(await qa(witness, chacha, "chacha.ron", await environment({ ...trust, SANDBOX2_TLS_DEFECT: "chacha-load" })), ["tls-chacha-load"], "tls-chacha-load");
      judge(await qa(witness, aes, "required.ron", await environment({ ...trust, SANDBOX2_TLS_DEFECT: "tls-name" })), ["tls-chacha-required"], "tls-chacha-required");
    }
  }
} finally {
  for (const child of children) if (child.exitCode === null) child.kill("SIGKILL");
  for (const socket of sockets) socket.destroy();
  await Promise.all(servers.map(server => new Promise<void>(resolve => server.close(() => resolve()))));
  await rm(work, { recursive: true, force: true });
}
