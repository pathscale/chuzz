import { mkdtemp, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

const repository = resolve(import.meta.dir, "..");
const fixture = join(repository, "apps/chuzz/src/reactor/qa");
const host = process.env.QA_HOST_BIN || join(repository, "target/release/chuzz-headless");
const witness = process.env.CHUZZ_REACTOR_WITNESS_BIN || join(repository, "target/release/chuzz-reactor-witness");
const control = process.env.CHUZZ_CONTROL_WITNESS_BIN || join(repository, "target/release/control-reactor-witness");
const qa = process.env.PS_QA_BIN || join(dirname(repository), "ps-observability/target/release/ps-qa");

async function execute(command: string[], env: Record<string, string | undefined>) {
  const child = Bun.spawn(command, { env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const [code, stdout, stderr] = await Promise.race([
      Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => { child.kill(); reject(new Error(`${command[0]} witness exceeded 90s`)); }, 90000);
      }),
    ]);
    return { code, stdout, stderr };
  } finally {
    clearTimeout(timer);
    if (child.exitCode === null) { child.kill(); await child.exited; }
  }
}

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
    if (character === "," && !quoted) { values.push(line.slice(start, index).trim()); start = index + 1; }
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
  for (const column of ["id", "verdict", "error"]) {
    if (!columns.includes(column)) { throw new Error(`missing TOON column ${column}`); }
  }
  const lines = stdout.slice(header.index! + header[0].length).split(/\r?\n/).slice(1, count + 1);
  if (lines.length !== count) { throw new Error("truncated checks table"); }
  return lines.map(line => {
    const values = cells(line.trim());
    if (values.length !== columns.length) { throw new Error("wrong TOON row width"); }
    return Object.fromEntries(columns.map((column, index) => [column, values[index]]));
  });
}

const work = await mkdtemp(join(tmpdir(), "chuzz-reactor-witness-"));
try {
  let sequence = 0;
  async function runLinked(binary: string, defect?: string, failure?: string) {
    const directory = join(work, String(sequence++));
    await mkdir(directory, { recursive: true });
    const command = binary === witness
      ? [binary, "--profile", directory, "--host", host, "--site", fixture, "--hang", join(fixture, "hang.html")]
      : [binary];
    const result = await execute(command, {
      ...process.env, CHUZZ_CONTROL_DIR: join(directory, "control"), CHUZZ_REACTOR_DEFECT: defect,
      CHUZZ_HEADLESS_MODE: "qa", CHUZZ_HEADLESS_WORKER_ROUTE: undefined,
      CHUZZ_HEADLESS_PAGE_WORKER: undefined, QA_HOST_PATH: undefined,
    });
    const text = result.stdout + result.stderr;
    if (failure ? result.code === 0 || !text.includes(failure) : result.code !== 0 || !text.includes(": passed")) {
      throw new Error(`${defect || binary}: wrong linked witness result ${result.code}\n${text}`);
    }
    console.log(`PASS: ${defect ? `${defect} rejected by ${failure}` : binary}`);
  }
  await runLinked(control);
  await runLinked(control, "control-listener", "control-reply");
  await runLinked(witness);
  for (const [defect, failure] of [
    ["page-ring-wakeup", "page-ring-wakeup"],
    ["egress-accept", "egress-accept"],
    ["egress-connection-limit", "egress-connection-limit"],
    ["egress-client-forward", "egress-client-forward"],
    ["egress-upstream-forward", "egress-upstream-forward"],
    ["egress-byte-cap", "egress-byte-cap"],
    ["cookie-snapshot-drop", "cookie-debounce"],
    ["wall-timeout", "wall-timeout"],
    ["descriptor-pipe-read", "stdout-announce"],
    ["supervisor-deadline", "supervisor-deadline"],
  ]) { await runLinked(witness, defect, failure); }

  async function runQa(page: string, checks: string, profile: string, id: string, defect?: string) {
    const directory = join(work, String(sequence++));
    await mkdir(directory, { recursive: true });
    const result = await execute([qa, "qa-hosted", "--host", host, "--page", page, "--app", profile, "--checks", checks], {
      ...process.env, PS_QA_HEADLESS: "true", CHUZZ_PROFILE_DIR: directory, CHUZZ_CONTROL_DIR: join(directory, "control"),
      CHUZZ_REACTOR_DEFECT: defect, CHUZZ_HEADLESS_MODE: "qa", CHUZZ_HEADLESS_WORKER_ROUTE: undefined,
      CHUZZ_HEADLESS_PAGE_WORKER: undefined, QA_HOST_PATH: undefined,
    });
    const rows = report(result.stdout);
    if (!rows.some(row => row.id === id)) { throw new Error(`${id}: check missing`); }
    if (defect) {
      if (result.code === 0 || !rows.some(row => row.id === id && row.verdict === "fail")) {
        throw new Error(`${defect}: planted defect did not fail ${id}\n${result.stdout}\n${result.stderr}`);
      }
    } else if (result.code !== 0 || rows.some(row => row.verdict !== "pass")) {
      throw new Error(`${id}: positive functional witness failed\n${result.stdout}\n${result.stderr}`);
    }
    console.log(`PASS: ${defect ? `${defect} rejected by ${id}` : id}`);
  }
  await runQa(fixture, join(fixture, "checks/page.ron"), join(fixture, "ps-qa.ron"), "page-control-arc");
  const platform = join(repository, "apps/chuzz/src/platform/qa");
  await runQa(join(platform, "webauthn.html"), join(platform, "checks/webauthn.ron"), join(platform, "ps-qa.ron"), "webauthn");
  await runQa(join(platform, "webauthn.html"), join(platform, "checks/webauthn.ron"), join(platform, "ps-qa.ron"), "webauthn", "webauthn-notification");
  const canvas = join(repository, "apps/chuzz/src/webgpu/canvas2d/qa");
  await runQa(join(canvas, "native-sources.html"), join(canvas, "checks/native-sources.ron"), join(canvas, "ps-qa.ron"), "canvassrc-native-sources");
  await runQa(join(canvas, "native-sources.html"), join(canvas, "checks/native-sources.ron"), join(canvas, "ps-qa.ron"), "canvassrc-native-sources", "canvas-readback-drop");
} finally {
  await rm(work, { recursive: true, force: true });
}
