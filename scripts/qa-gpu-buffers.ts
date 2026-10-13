import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

type Child = Bun.Subprocess<"ignore", "pipe", "pipe">;
type Service = { child: Child; name: string; log: string[]; drained: Promise<void> };
type Case = { mode: string; checks: string; ids: string[]; failure?: string; label?: string };

const repository = resolve(import.meta.dir, "..");
const fixtureDirectory = join(repository, "apps/chuzz/src/webgpu/qa/buffers");
const host = process.env.QA_HOST_BIN || join(repository, "target/release/chuzz-headless");
const fixture = process.env.QA_FIXTURE || join(repository, "target/release/chuzz-qa-fixture");
const qa = process.env.PS_QA_BIN || join(dirname(repository), "ps-observability/target/release/ps-qa");
const children: Child[] = [];
const services: Service[] = [];

function port(name: string, fallback: number): number {
  const value = Number(process.env[name] || fallback);
  if (!Number.isInteger(value) || value < 1 || value > 65535) {
    throw new Error(`${name} must be a TCP port`);
  }
  return value;
}

function launch(command: string[], env: Record<string, string | undefined>): Child {
  const child = Bun.spawn(command, { env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  children.push(child);
  return child;
}

async function deadline<T>(promise: Promise<T>, ms: number, name: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${name} exceeded ${ms}ms`)), ms);
      })
    ]);
  } finally {
    clearTimeout(timer);
  }
}

async function capture(
  stream: ReadableStream<Uint8Array>, log: string[], onLine?: (line: string) => void
): Promise<void> {
  const reader = stream.getReader();
  const decoder = new TextDecoder();
  let pending = "";
  try {
    for (;;) {
      const chunk = await reader.read();
      const text = chunk.done ? decoder.decode() : decoder.decode(chunk.value, { stream: true });
      log.push(text);
      pending += text;
      let newline;
      while ((newline = pending.indexOf("\n")) >= 0) {
        onLine?.(pending.slice(0, newline).replace(/\r$/, ""));
        pending = pending.slice(newline + 1);
      }
      if (chunk.done) {
        if (pending) { onLine?.(pending); }
        return;
      }
    }
  } finally {
    reader.releaseLock();
  }
}

async function startService(
  name: string, command: string[], marker: string, env: Record<string, string | undefined>
): Promise<void> {
  const child = launch(command, env);
  const log: string[] = [];
  const ready = Promise.withResolvers<void>();
  const drained = Promise.all([
    capture(child.stdout, log, line => { if (line.startsWith(marker)) { ready.resolve(); } }),
    capture(child.stderr, log)
  ]).then(() => {});
  services.push({ child, name, log, drained });
  void drained.catch(error => ready.reject(error));
  void child.exited.then(code => ready.reject(new Error(`${name} exited ${code} before readiness`)));
  await deadline(ready.promise, 30000, `${name} readiness`);
  if (child.exitCode !== null) { throw new Error(`${name} exited during startup`); }
}

// ps-qa's checks table is comma-delimited TOON. Quoted cells use JSON escapes.
function cells(line: string): string[] {
  const values: string[] = [];
  let start = 0;
  let quoted = false;
  let escaped = false;
  for (let index = 0; index < line.length; index++) {
    const character = line[index];
    if (escaped) { escaped = false; continue; }
    if (quoted && character === "\\") { escaped = true; continue; }
    if (character === '"') { quoted = !quoted; }
    if (character === "," && !quoted) {
      values.push(line.slice(start, index).trim());
      start = index + 1;
    }
  }
  if (quoted || escaped) { throw new Error("unterminated TOON cell"); }
  values.push(line.slice(start).trim());
  return values.map(value => value.startsWith('"') ? String(JSON.parse(value)) : value);
}

function report(stdout: string): Record<string, string>[] {
  const headers = [...stdout.matchAll(/^checks\[(\d+)\]\{([^}]+)\}:\r?$/gm)];
  if (headers.length !== 1) { throw new Error("expected one ps-qa checks table"); }
  const header = headers[0];
  const count = Number(header[1]);
  const columns = header[2].split(",").map(value => value.trim());
  for (const required of ["verdict", "id", "error"]) {
    if (!columns.includes(required)) { throw new Error(`missing TOON column ${required}`); }
  }
  const lines = stdout.slice(header.index! + header[0].length).split(/\r?\n/).slice(1, count + 1);
  if (lines.length !== count) { throw new Error("truncated ps-qa checks table"); }
  return lines.map(line => {
    if (!line.startsWith("  ")) { throw new Error("missing ps-qa check row"); }
    const values = cells(line.trim());
    if (values.length !== columns.length) { throw new Error("wrong TOON row width"); }
    return Object.fromEntries(columns.map((column, index) => [column, values[index]]));
  });
}

const inspection = "gpu-buffers-inspection";
const ready = "gpu-buffers-ready";
const errors = "gpu-buffers-no-errors";
const range = "gpu-buffers-particles-range";
const counter = "gpu-buffers-counter";
const vertices = "gpu-buffers-indirect-vertices";
const instances = "gpu-buffers-indirect-instances";
const changes = "gpu-buffers-particles-changes";
const all = [inspection, ready, errors, range, counter, vertices, instances, changes];
const cases: Case[] = [
  { mode: "normal", checks: "checks/buffers.ron", ids: all },
  { mode: "nan", checks: "negative/nan.ron", ids: [inspection, ready, errors, counter, vertices, instances, range],
    failure: range, label: "qa-particles" },
  { mode: "skip", checks: "negative/skip.ron", ids: [inspection, ready, errors, range, counter],
    failure: counter, label: "qa-counter" },
  { mode: "freeze", checks: "negative/freeze.ron", ids: all,
    failure: changes, label: "qa-particles" }
];

function verify(witness: Case, code: number, stdout: string): void {
  const rows = report(stdout);
  const ids = new Set(rows.map(row => row.id));
  if (rows.length !== witness.ids.length || ids.size !== rows.length ||
      witness.ids.some(id => !ids.has(id))) {
    throw new Error(`${witness.mode}: report does not contain exactly the expected checks`);
  }
  for (const row of rows) {
    const verdict = row.id === witness.failure ? "fail" : "pass";
    if (row.verdict !== verdict) {
      throw new Error(`${witness.mode}: ${row.id} expected ${verdict}, got ${row.verdict}: ${row.error}`);
    }
    if (verdict === "fail" && (!row.error.includes(witness.label!) || !/\bsubmits?\b/i.test(row.error))) {
      throw new Error(`${witness.mode}: failure lacks the contracted buffer label and submit diagnostic`);
    }
    if (verdict === "fail" && witness.mode === "nan" && !/\bNaN\b/i.test(row.error)) {
      throw new Error("nan: expected a NaN diagnostic from the particle range check");
    }
  }
  if (code !== (witness.failure ? 1 : 0)) {
    throw new Error(`${witness.mode}: unexpected ps-qa exit code ${code}`);
  }
}

async function stop(child: Child): Promise<void> {
  if (child.exitCode !== null) { return; }
  child.kill("SIGTERM");
  try {
    await deadline(child.exited, 5000, "child shutdown");
  } catch {
    if (child.exitCode === null) { child.kill("SIGKILL"); }
    await child.exited;
  }
}

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.once(signal, () => {
    for (const child of children) { if (child.exitCode === null) { child.kill("SIGTERM"); } }
  });
}

const work = await mkdtemp(join(tmpdir(), "chuzz-gpu-buffers-"));
try {
  const frontPort = port("QA_GPU_FRONT_PORT", 49123);
  const upstreamPort = port("QA_GPU_UPSTREAM_PORT", 49124);
  if (frontPort === upstreamPort) { throw new Error("front and upstream ports must differ"); }
  const tlsDirectory = join(work, "tls");
  await mkdir(tlsDirectory, { mode: 0o700 });
  const env = { ...process.env, SSL_CERT_FILE: join(tlsDirectory, "ca.pem") };
  await startService("fixture", [fixture, "--port", String(upstreamPort), "--front-port", String(frontPort)],
    `QA fixture: https://127.0.0.1:${frontPort}/`, env);
  await startService("h2 front", [process.execPath, join(repository, "scripts/qa-h2-front.ts"),
    "--dir", tlsDirectory, "--listen", String(frontPort), "--upstream", String(upstreamPort)],
    `h2 front https://127.0.0.1:${frontPort}/ -> ${upstreamPort}`, env);

  for (const witness of cases) {
    const profile = join(work, witness.mode);
    const control = join(profile, "control");
    await mkdir(control, { recursive: true, mode: 0o700 });
    const query = witness.mode === "normal" ? "" : `?break=${witness.mode}`;
    const page = `https://127.0.0.1:${frontPort}/webgpu/buffers/index.html${query}`;
    const child = launch([qa, "qa-hosted", "--host", host, "--page", page,
      "--app", join(fixtureDirectory, "ps-qa.ron"), "--checks", join(fixtureDirectory, witness.checks)],
      { ...env, PS_QA_HEADLESS: "true", CHUZZ_PROFILE_DIR: profile, CHUZZ_CONTROL_DIR: control });
    const run = Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
    const [code, stdout, stderr] = await deadline(Promise.race([
      run,
      ...services.map(service => service.child.exited.then(code => {
        throw new Error(`${service.name} exited ${code} while running ${witness.mode}`);
      }))
    ]), 120000, `${witness.mode} witness`);
    process.stdout.write(stdout);
    process.stderr.write(stderr);
    verify(witness, code, stdout);
    console.log(witness.failure
      ? `PASS: ?break=${witness.mode} rejected by ${witness.failure}`
      : "PASS: normal GPU buffer fixture passed all eight checks");
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  for (const service of services) { console.error(`${service.name} log:\n${service.log.join("")}`); }
  process.exitCode = 1;
} finally {
  await Promise.all(children.map(stop));
  await Promise.allSettled(services.map(service => service.drained));
  await rm(work, { recursive: true, force: true });
}
