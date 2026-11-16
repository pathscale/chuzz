import { readFile, writeFile, mkdir, copyFile } from "node:fs/promises";
import path from "node:path";

// bun scripts/pays-wallet-budget-repro.ts DESCRIPTOR PAYS_ROOT NEW_EVIDENCE_DIR
// PS_QA_BIN selects the existing ps-qa binary. Host lifecycle belongs to caller.
const [descriptor, paysRoot, output] = Bun.argv.slice(2);
if (![descriptor, paysRoot, output].every(value => value && path.isAbsolute(value))) {
  throw new Error("Supply absolute descriptor, Pays checkout, and new evidence directory");
}
if (["/tmp", "/private/tmp"].some(root => output === root || output.startsWith(root + "/"))) {
  throw new Error("Evidence must live in the visible private task directory");
}
await mkdir(output, { mode: 0o700 });
await writeFile(path.join(output, "OWNER.md"),
  "Pays wallet budget replay evidence. Delete after coordinator diagnosis and verification.\n",
  { mode: 0o600, flag: "wx" });
const checks = path.join(output, "checks");
await mkdir(checks, { mode: 0o700 });
const fixture = path.join(paysRoot, "tests/ps-qa/local-full-qa");
for (const file of ["01-public-and-checkout.ron", "02-merchant-portal.ron"]) {
  // Copy the existing assertions byte for byte; do not create a second oracle.
  await copyFile(path.join(fixture, "checks", file), path.join(checks, file));
}
const bin = process.env.PS_QA_BIN || "ps-qa";
const common = ["--headless", "--descriptor", descriptor,
  "--app", path.join(fixture, "ps-qa.ron")];
async function command(name: string, args: string[], milliseconds: number) {
  const child = Bun.spawn([bin, ...args], { stdout: "pipe", stderr: "pipe" });
  let timedOut = false;
  const timer = setTimeout(() => { timedOut = true; child.kill("SIGKILL"); }, milliseconds);
  try {
    const [stdout, stderr, code] = await Promise.all([
      new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
    ]);
    await writeFile(path.join(output, name + ".stdout.log"), stdout, { mode: 0o600 });
    await writeFile(path.join(output, name + ".stderr.log"), stderr, { mode: 0o600 });
    return { code, timedOut, stdout, stderr };
  } finally { clearTimeout(timer); }
}
const listing = await command("list", ["list", "--checks", checks], 15000);
if (listing.code !== 0 || listing.timedOut) throw new Error("Cannot list original checks");
const ids = [...listing.stdout.matchAll(/^  ([a-zA-Z0-9_-]+) /gm)].map(match => match[1]);
const target = "local-merchant-wallet-invalid-purpose-refused";
const index = ids.indexOf(target);
if (index !== 19 || new Set(ids).size !== ids.length) {
  throw new Error("Expected the recorded 19-check prefix; review fixture changes explicitly");
}
const results: object[] = [];
type WorkflowState = {
  id: string;
  passed: boolean;
  diagnostics: Awaited<ReturnType<typeof command>>;
};
let lastSuccessful: WorkflowState | null = null;
let firstFailed: { previous: WorkflowState | null; current: WorkflowState } | null = null;
async function save() {
  await writeFile(path.join(output, "result.json"), JSON.stringify(results, null, 2) + "\n",
    { mode: 0o600 });
}
async function qa(id: string) {
  const result = await command(id, [...common, "qa", "--checks", checks, id], 180000);
  results.push({ kind: "qa", id, ...result });
  await save();
  const passed = result.code === 0 && !result.timedOut;
  // Host-owned runtime-error batches survive termination. Do not replace this
  // with eval or __chuzzRuntimeDiagnostics, which enters the failed realm.
  const diagnostics = await command(id + ".host-watch", [...common, "watch", "0.2"], 15000);
  const current: WorkflowState = { id, passed, diagnostics };
  if (passed) lastSuccessful = current;
  else if (firstFailed === null) firstFailed = { previous: lastSuccessful, current };
  results.push({ kind: "host-workflow-state", ...current });
  await save();
  await writeFile(path.join(output, "workflow-boundary.json"),
    JSON.stringify({ lastSuccessful, firstFailed }, null, 2) + "\n", { mode: 0o600 });
  return passed;
}
for (const id of ids.slice(0, index)) {
  if (!await qa(id)) throw new Error(`Precondition failed: ${id}; evidence in ${output}`);
}
const validationPassed = await qa(target);
// Match the original failure capture ordering, without treating a tree as DOM.
const tree = await command("after-invalid.tree", [...common, "tree"], 15000);
results.push({ kind: "semantic-tree-only", ...tree });
await save();
const dom = await command("after-invalid.dom",
  [...common, "eval", "document.documentElement.outerHTML"], 15000);
const domAvailable = dom.code === 0 && !dom.timedOut;
results.push({ kind: "dom", available: domAvailable, ...dom });
await save();
// Probe a real navigation, skipping stale readback assertions after the failure.
const navigationPassed = await qa("local-merchant-applications-renders");
results.push({ kind: "summary", validationPassed, domAvailable, navigationPassed });
await save();
if (!validationPassed || !domAvailable || !navigationPassed) {
  throw new Error(`Wallet validation/DOM/navigation regression; evidence in ${output}`);
}
