import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";

type Result = { code: number; stdout: string; stderr: string };
type Row = Record<string, string>;
const repository = resolve(import.meta.dir, "..");
const fixtures = join(repository, "apps/chuzz/src/qa/wasm-retirement");
const qa = process.env.PS_QA_BIN || join(dirname(repository), "ps-observability/target/release/ps-qa");
const gui = process.env.QA_WASM_GUI_BIN || join(repository, "target/release/chuzz-gui");
const host = process.env.QA_HOST_BIN || join(repository, "target/release/chuzz-headless");
const minimalGui = process.env.QA_WASM_MINIMAL_GUI_BIN || join(repository, "target/qa-s11/minimal/chuzz-gui");
const flagGui = process.env.QA_WASM_DEFECT_GUI_BIN || join(repository, "target/qa-s11/wasm-flag-name/chuzz-gui");
const flagHost = process.env.QA_WASM_DEFECT_HOST_BIN || join(repository, "target/qa-s11/wasm-flag-name/chuzz-headless");
const captureGui = process.env.QA_CAPTURE_WASM_DEFECT_GUI_BIN || join(repository, "target/qa-s11/capture-wasm-flag-name/chuzz-gui");
const captureHost = process.env.QA_CAPTURE_WASM_DEFECT_HOST_BIN || join(repository, "target/qa-s11/capture-wasm-flag-name/chuzz-headless");
const globalHost = process.env.QA_WEBASSEMBLY_DEFECT_HOST_BIN || join(repository, "target/qa-s11/webassembly-global/chuzz-headless");
const children = new Set<Bun.Subprocess>();
const maxBytes = 4 * 1024 * 1024;

async function bounded(stream: ReadableStream<Uint8Array>): Promise<string> {
    const reader = stream.getReader();
    const chunks: Uint8Array[] = [];
    let bytes = 0;
    try {
        for (;;) {
            const next = await reader.read();
            if (next.done) break;
            bytes += next.value.length;
            if (bytes > maxBytes) throw new Error("witness output exceeds 4 MiB");
            chunks.push(next.value);
        }
        const combined = new Uint8Array(bytes);
        let offset = 0;
        for (const chunk of chunks) { combined.set(chunk, offset); offset += chunk.length; }
        return new TextDecoder().decode(combined);
    } finally { reader.releaseLock(); }
}

async function run(command: string[], env: Record<string, string | undefined>, timeout: number): Promise<Result> {
    const child = Bun.spawn(command, { env, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
    children.add(child);
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
        const [code, stdout, stderr] = await Promise.race([
            Promise.all([child.exited, bounded(child.stdout), bounded(child.stderr)]),
            new Promise<never>((_, reject) => {
                timer = setTimeout(() => reject(new Error(`${command[0]} exceeded ${timeout}ms`)), timeout);
            }),
        ]);
        return { code, stdout, stderr };
    } finally {
        clearTimeout(timer);
        if (child.exitCode === null) { child.kill("SIGKILL"); await child.exited; }
        children.delete(child);
    }
}

function cells(line: string): string[] {
    const values: string[] = [];
    let quoted = false;
    let escaped = false;
    let start = 0;
    for (let index = 0; index < line.length; index++) {
        const char = line[index];
        if (escaped) { escaped = false; continue; }
        if (quoted && char === "\\") { escaped = true; continue; }
        if (char === '"') { quoted = !quoted; continue; }
        if (char === "," && !quoted) { values.push(line.slice(start, index).trim()); start = index + 1; }
    }
    if (quoted || escaped) throw new Error("malformed ps-qa TOON cell");
    values.push(line.slice(start).trim());
    return values.map(value => value.startsWith('"') ? String(JSON.parse(value)) : value);
}

function rows(text: string): Row[] {
    const headers = [...text.matchAll(/^checks\[(\d+)\]\{([^}]+)\}:\r?$/gm)];
    if (headers.length !== 1) throw new Error("expected exactly one ps-qa checks table");
    const header = headers[0];
    const count = Number(header[1]);
    const columns = header[2].split(",").map(value => value.trim());
    if (!["id", "verdict", "error"].every(column => columns.includes(column))) throw new Error("missing ps-qa verdict columns");
    const lines = text.slice(header.index! + header[0].length).split(/\r?\n/).slice(1, count + 1);
    if (lines.length !== count) throw new Error("truncated ps-qa checks table");
    return lines.map(line => {
        if (!line.startsWith("  ")) throw new Error("missing ps-qa check row");
        const values = cells(line.trim());
        if (values.length !== columns.length) throw new Error("invalid ps-qa row width");
        return Object.fromEntries(columns.map((column, index) => [column, values[index]]));
    });
}

function escape(text: string): string {
    return text.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
}

const ids = ["wasm-cli-refusal", "capture-wasm-cli-refusal"];
const markers = ["PASS: wasm-unsupported --wasm exit 2", "PASS: wasm-unsupported --capture-wasm exit 2"];

async function cliPage(binaries: string[], directory: string): Promise<string> {
    await mkdir(directory, { recursive: true });
    const outcomes = [true, true];
    const observations: object[] = [];
    for (const binary of binaries) {
        for (const [index, flag] of ["--wasm", "--capture-wasm"].entries()) {
            const module = join(directory, "missing-module.wasm");
            const png = join(directory, "forbidden.png");
            const tree = join(directory, "forbidden.txt");
            const cases = [
                [flag],
                [flag, module, "--out", png, "--tree", tree],
                [`${flag}=${module}`, "--out", png],
                ["--capture", png, flag],
            ];
            for (const args of cases) {
                const result = await run([binary, ...args], { ...process.env }, 10000);
                const outputsAbsent = !await Bun.file(png).exists() && !await Bun.file(tree).exists();
                const named = /\bwasm-unsupported\b/.test(result.stderr);
                const passed = result.code === 2 && named && result.stdout.trim() === "" && outputsAbsent;
                outcomes[index] &&= passed;
                observations.push({ binary, args, ...result, outputsAbsent, passed });
            }
        }
    }
    await writeFile(join(directory, "cli-observations.json"), JSON.stringify(observations, null, 2) + "\n");
    const page = join(directory, "index.html");
    // Only successful real CLI observations produce the asserted marker.
    await writeFile(page, '<!doctype html><html><head><meta charset="utf-8"><title>Wasm CLI refusal</title></head><body>' +
        '<h1>Wasm CLI refusal</h1>' + outcomes.map((passed, index) =>
            `<p>${escape(passed ? markers[index] : `FAIL: ${ids[index]} see cli-observations.json`)}</p>`).join("") +
        '</body></html>\n');
    return page;
}

async function hosted(label: string, binary: string, page: string, checks: string, expected: string[], failure?: string): Promise<void> {
    const profile = join(work, label, "profile");
    const control = join(profile, "control");
    await mkdir(control, { recursive: true, mode: 0o700 });
    const result = await run([qa, "qa-hosted", "--host", binary, "--page", page,
        "--app", join(fixtures, "ps-qa.ron"), "--checks", join(fixtures, checks)],
        { ...process.env, PS_QA_HEADLESS: "true", CHUZZ_PROFILE_DIR: profile, CHUZZ_CONTROL_DIR: control }, 60000);
    await writeFile(join(work, `${label}.stdout.toon`), result.stdout);
    await writeFile(join(work, `${label}.stderr.txt`), result.stderr);
    const report = rows(result.stdout);
    const actual = new Set(report.map(row => row.id));
    if (report.length !== expected.length || actual.size !== expected.length || expected.some(id => !actual.has(id))) {
        throw new Error(`${label}: wrong check IDs`);
    }
    for (const row of report) {
        const verdict = row.id === failure ? "fail" : "pass";
        if (row.verdict !== verdict || (verdict === "fail" && !row.error.trim())) {
            throw new Error(`${label}: ${row.id} expected ${verdict}, got ${row.verdict}: ${row.error}`);
        }
    }
    if (result.code !== (failure ? 1 : 0)) throw new Error(`${label}: unexpected ps-qa exit ${result.code}`);
    console.log(failure ? `PASS: ${label} failed by name ${failure}` : `PASS: ${label}`);
}

for (const binary of [qa, gui, host, minimalGui, flagGui, flagHost, captureGui, captureHost, globalHost]) {
    if (!await Bun.file(binary).exists()) throw new Error(`Missing prebuilt witness binary: ${binary}`);
}
const workRoot = resolve(process.env.QA_WASM_OUTPUT_DIR || join(repository, "target/qa-s11/evidence"));
await mkdir(workRoot, { recursive: true });
const work = await mkdtemp(join(workRoot, "run-"));
console.log(`Evidence: ${work}`);
for (const signal of ["SIGINT", "SIGTERM"] as const) {
    process.once(signal, () => { for (const child of children) child.kill("SIGKILL"); process.exit(130); });
}
try {
    await hosted("cli-normal", host, await cliPage([gui, host, minimalGui], join(work, "cli-normal")), "cli.ron", ids);
    await hosted("wasm-flag-name", host, await cliPage([flagGui, flagHost], join(work, "wasm-flag-name")), "cli.ron", ids, ids[0]);
    await hosted("capture-wasm-flag-name", host, await cliPage([captureGui, captureHost], join(work, "capture-wasm-flag-name")), "cli.ron", ids, ids[1]);
    const page = join(fixtures, "index.html");
    await hosted("page-normal", host, page, "page.ron", ["webassembly-reference-error"]);
    await hosted("webassembly-global", globalHost, page, "page.ron", ["webassembly-reference-error"], "webassembly-reference-error");
} catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
} finally {
    for (const child of children) { if (child.exitCode === null) child.kill("SIGKILL"); }
    await Promise.all([...children].map(child => child.exited));
}
