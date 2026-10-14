import { watch } from "node:fs";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { Rpc } from "./qa-tab-rpc";
import { defects } from "./qa-tab-defects";

const repository = resolve(import.meta.dir, "..");
const fixtures = join(repository, "apps/chuzz/src/tab_lifecycle/qa");
const qa = process.env.PS_QA_BIN || join(dirname(repository), "ps-observability/target/release/ps-qa");
const ordinary = process.env.QA_HOST_BIN || join(repository, "target/release/chuzz-gui");
const manifestPath = process.env.QA_TAB_MUTANTS;
if (!manifestPath) throw new Error("QA_TAB_MUTANTS must name the six prebuilt production mutants from qa-tab-defects.ts");
const binaries = await Bun.file(manifestPath).json() as Record<string, string>;
const delay = 300000;
const html = await Bun.file(join(fixtures, "fixture.html")).text();
const worker = await Bun.file(join(fixtures, "worker.ts")).text();
const cases = [
  { mode: "sleep", id: "stash-teardown-complete", subject: "Sleeping, saved view", chrome: true },
  { mode: "form", id: "stash-sensitive-empty", subject: "READY: FORM RESTORED", chrome: false },
  { mode: "origin", id: "stash-origin-before-script", subject: "READY: ORIGIN ISOLATED", chrome: false },
  { mode: "post", id: "tab-restore-failure-repost", subject: "reload-requires-resubmission", chrome: true },
  { mode: "visible", id: "tab-visible-never-sleeps", subject: "Sleeping, saved view", chrome: true },
  { mode: "hidden", id: "tab-five-minute-expiry", subject: "QA hidden (Sleeping)", chrome: true },
];

function deadline<T>(work: Promise<T>, milliseconds: number, name: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout>;
  return Promise.race([work, new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error(name)), milliseconds); })]).finally(() => clearTimeout(timer));
}

function cells(line: string): string[] {
  let quoted = false, escaped = false, start = 0;
  const out: string[] = [];
  for (let index = 0; index < line.length; index++) {
    const character = line[index];
    if (escaped) { escaped = false; continue; }
    if (quoted && character === "\\") { escaped = true; continue; }
    if (character === '"') { quoted = !quoted; continue; }
    if (character === "," && !quoted) { out.push(line.slice(start, index).trim()); start = index + 1; }
  }
  if (quoted || escaped) throw new Error("invalid TOON quoted cell");
  out.push(line.slice(start).trim());
  return out.map(value => value.startsWith('"') ? String(JSON.parse(value)) : value);
}

function verdict(stdout: string, id: string): Record<string, string> {
  const headers = [...stdout.matchAll(/^checks\[(\d+)\]\{([^}]+)\}:\r?$/gm)];
  if (headers.length !== 1 || Number(headers[0][1]) !== 1) throw new Error("missing or ambiguous ps-qa verdict");
  const header = headers[0];
  const columns = header[2].split(",").map(value => value.trim());
  const line = stdout.slice(header.index! + header[0].length).split(/\r?\n/)[1];
  if (!line?.startsWith("  ")) throw new Error("missing ps-qa check row");
  const values = cells(line.trim());
  if (values.length !== columns.length) throw new Error("invalid ps-qa row width");
  const row = Object.fromEntries(columns.map((name, index) => [name, values[index]]));
  if (row.id !== id || !["pass", "fail"].includes(row.verdict)) throw new Error("wrong declared check");
  return row;
}

async function check(descriptor: string, id: string, chrome: boolean, mustFail = false): Promise<void> {
  const command = [qa, "--descriptor", descriptor, chrome ? "--chrome" : "--tab-id", ...(chrome ? [] : ["0"]),
    "--require-paint-events", "--app", join(fixtures, "ps-qa.ron"), "qa", id, "--checks", join(fixtures, "checks")];
  const child = Bun.spawn(command, { stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const [code, stdout, stderr] = await deadline(Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]), 25000, `${id}: ps-qa did not return`);
  const row = verdict(stdout, id);
  if (code !== (mustFail ? 1 : 0) || row.verdict !== (mustFail ? "fail" : "pass")) throw new Error(`${id}: unexpected ${row.verdict}: ${row.error}\n${stderr}`);
  if (mustFail && /targetUnavailable|unsupportedRequest|socket|descriptor|crashed|transport|backend unavailable/i.test(row.error)) throw new Error(`${id}: infrastructure failure is not the planted outcome`);
  if (mustFail && !row.error.includes(cases.find(item => item.id === id)!.subject)) throw new Error(`${id}: failure does not identify the declared rendered subject`);
  process.stdout.write(stdout);
}

async function stop(child: Bun.Subprocess): Promise<void> {
  if (child.exitCode !== null) return;
  child.kill("SIGTERM");
  try { await deadline(child.exited, 5000, "GUI shutdown deadline"); }
  catch { if (child.exitCode === null) child.kill("SIGKILL"); await child.exited; }
}

async function run(mode: typeof cases[number], binary: string, defect?: string): Promise<void> {
  const directory = await mkdtemp(join(tmpdir(), "chuzz-tab-witness-"));
  const privateTmp = join(directory, "tmp");
  const descriptors = join(privateTmp, "tauri-blitz-agent");
  await mkdir(descriptors, { recursive: true, mode: 0o700 });
  let redirect = false, posts = 0, beats = 0;
  const serve = (destination: boolean) => Bun.serve({ hostname: "127.0.0.1", port: 0, async fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/worker.js") return new Response(worker, { headers: { "content-type": "text/javascript" } });
    if (url.pathname === "/beat") { beats++; return new Response("ok"); }
    if (url.pathname === "/slow") { await Bun.sleep(1000); return new Response("completed"); }
    if (request.method === "POST") posts++;
    if (redirect && !destination && url.pathname === "/fixture") return Response.redirect(`${other.url}fixture?case=origin`, 302);
    return new Response(html, { headers: { "content-type": "text/html", "cache-control": "no-store" } });
  }});
  const other = serve(true);
  const front = serve(false);
  const ready = Promise.withResolvers<string>();
  let child: Bun.Subprocess | undefined;
  const watcher = watch(descriptors, (_, filename) => {
    if (!filename?.toString().endsWith(".json")) return;
    const path = join(descriptors, filename.toString());
    void Bun.file(path).json().then(value => { if (value.pid === child?.pid && value.protocolVersion === 2) ready.resolve(path); }).catch(() => {});
  });
  let rpc: Rpc | undefined;
  try {
    child = Bun.spawn([binary, `${front.url}fixture?case=${mode.mode}`], { stdin: "ignore", stdout: "pipe", stderr: "pipe",
      env: { ...process.env, TMPDIR: privateTmp, CHUZZ_PROFILE_DIR: join(directory, "profile"), CHUZZ_TAB_SLEEP_MS: String(delay),
        CHUZZ_CONTROL: "1" } });
    const logs = Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text()]);
    const descriptor = await deadline(Promise.race([ready.promise, child.exited.then(code => { throw new Error(`preparation GUI exited ${code}`); })]), 20000, "GUI descriptor deadline");
    watcher.close();
    const advertised = await Bun.file(descriptor).json();
    rpc = await Rpc.open(advertised.address.replace(/^unix:\/\//, ""));
    await rpc.subscribeTabs();
    const mounted = await rpc.tabsUntil(tabs => tabs.some(tab => tab.id === 0 && tab.lifecycle === "live"), 15000);
    if (!mounted.some(tab => tab.id === 0 && tab.lifecycle === "live")) throw new Error("preparation never mounted the fixture");
    await check(descriptor, "lifecycle-fixture-ready", false);
    for (const id of ["draft", "password", "otp", "off", "marked", "inherited"]) await rpc.act(0, id, "setValue", id === "draft" ? "ordinary draft" : `fixture-${id}`);
    await rpc.act(0, "prepare", "click");
    await check(descriptor, "lifecycle-fixture-prepared", false);
    if (mode.mode === "post") {
      await rpc.act(0, "submit", "click");
      await rpc.tabsUntil(tabs => tabs.some(tab => tab.id === 0 && !tab.status.includes("loading") && tab.url.includes("/posted")), 15000);
      await check(descriptor, "lifecycle-post-ready", false);
      if (posts !== 1) throw new Error("preparation did not make exactly one real POST");
    }
    if (mode.mode === "visible" || mode.mode === "hidden") {
      if (mode.mode === "hidden") await rpc.invoke("open_tab", { url: "about:blank" });
      await Bun.sleep(delay + 500);
      if (mode.mode === "hidden" && !defect) await rpc.tabsUntil(tabs => tabs.some(tab => tab.id === 0 && tab.lifecycle === "sleeping"), 3000);
    } else {
      await rpc.invoke("sleep_tab", { id: 0 });
      const tabs = await rpc.tabsUntil(tabs => tabs.some(tab => tab.id === 0
        && (tab.lifecycle === "sleeping" || defect === "stash-retain-worker-owner" && tab.lifecycle === "discarding")), 10000);
      const sleeping = tabs.find(tab => tab.id === 0);
      if (!defect && (!sleeping?.runtimeReleased || sleeping.lifecycle !== "sleeping")) throw new Error("ordinary sleep lacks the native release receipt");
      if (!defect) {
        const quiet = beats;
        await Bun.sleep(500);
        if (beats !== quiet) throw new Error("sleep released metadata but retained real page/worker network activity");
      }
      if (mode.mode === "origin") redirect = true;
      if (mode.mode !== "sleep") {
        await rpc.invoke("wake_tab", { id: 0 });
        await rpc.tabsUntil(tabs => tabs.some(tab => tab.id === 0 && ["live", "restore-failed"].includes(tab.lifecycle)), 15000);
      }
    }
    await check(descriptor, mode.id, mode.chrome, !!defect);
    if (mode.mode === "post" && posts !== (defect ? 2 : 1)) throw new Error("POST witness did not observe the expected actual submission count");
    if (!defect && mode.mode === "visible") {
      const tab = (await rpc.evaluate("globalThis.__qaTabs")).find((tab: any) => tab.id === 0);
      if (tab.lifecycle !== "live" || !tab.visible || tab.runtimeReleased) throw new Error("viewed page did not remain resident and on screen");
    }
    console.log(`${mode.id}: ${defect ? `planted ${defect} failed by name` : "ordinary input passed"}; real deadline ${delay}ms`);
    rpc.close(); rpc = undefined;
    await stop(child);
    await logs;
  } finally {
    watcher.close(); rpc?.close();
    if (child) await stop(child);
    front.stop(true); other.stop(true);
    await rm(directory, { recursive: true, force: true });
  }
}

for (const item of cases) await run(item, ordinary);
for (const defect of defects) {
  const binary = binaries[defect.name];
  if (!binary || !await Bun.file(binary).exists()) throw new Error(`missing prebuilt production mutant: ${defect.name}`);
  const item = cases.find(item => item.id === defect.check);
  if (!item) throw new Error(`undeclared defect check: ${defect.check}`);
  await run(item, binary, defect.name);
}
