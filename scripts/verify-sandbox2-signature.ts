export async function command(argv: string[], input?: string): Promise<{ stdout: string; stderr: string }> {
  const child = Bun.spawn(argv, { stdin: input === undefined ? "ignore" : "pipe", stdout: "pipe", stderr: "pipe" });
  if (input !== undefined && child.stdin) { child.stdin.write(input); child.stdin.end(); }
  const [code, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  if (code !== 0) throw new Error(`${argv[0]} exited ${code}: ${stderr}`);
  return { stdout, stderr };
}

export async function verifySignature(binary: string): Promise<void> {
  await command(["/usr/bin/codesign", "--verify", "--strict", binary]);
  const details = await command(["/usr/bin/codesign", "-dv", "--verbose=4", binary]);
  if (!/flags=.*\bruntime\b/.test(details.stderr)) throw new Error("hardened-runtime: missing runtime code-signing flag");
  const entitlement = await command(["/usr/bin/codesign", "-d", "--entitlements", ":-", binary]);
  const converted = await command(["/usr/bin/plutil", "-convert", "json", "-o", "-", "-"], entitlement.stdout);
  const entitlements = JSON.parse(converted.stdout);
  const keys = Object.keys(entitlements);
  if (keys.length !== 1 || keys[0] !== "com.apple.security.cs.allow-jit" || entitlements[keys[0]] !== true) {
    throw new Error("entitlements-exact: allow-jit must be the only entitlement");
  }
}

if (import.meta.main) {
  const binary = Bun.argv[2];
  if (!binary) throw new Error("usage: bun scripts/verify-sandbox2-signature.ts BINARY");
  await verifySignature(binary);
  console.log("entitlements-exact PASS\nhardened-runtime PASS");
}
