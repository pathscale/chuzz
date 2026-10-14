import { cp, mkdir, rm, utimes } from "node:fs/promises";
import { join, resolve } from "node:path";
import { audit } from "../../scripts/audit-c-surface";
import { command, verifySignature } from "../../scripts/verify-sandbox2-signature";

const script = import.meta.dir;
const repository = resolve(script, "../..");
process.chdir(repository);
const args = Bun.argv.slice(2);
const profile = args[0] === "debug" || args[0] === "release" ? args.shift()! : "release";
for (let index = 0; index < args.length; index += 2) {
  if (args[index] !== "--config" || !args[index + 1]) {
    throw new Error("usage: build-app.ts [debug|release] [--config VALUE]...");
  }
}
if (process.platform !== "darwin") throw new Error("Chuzz.app requires macOS");
const manifest = await Bun.file(join(repository, "Cargo.toml")).text();
const version = manifest.match(/^\[workspace\.package\]\s*\n(?:.*\n)*?version\s*=\s*"([^"]+)"/m)?.[1];
if (!version) throw new Error("could not read workspace package version");
const bundle = join(repository, "target", profile, "bundle/macos/Chuzz.app");
const contents = join(bundle, "Contents");
const macos = join(contents, "MacOS");
const resources = join(contents, "Resources");

const metadataResult = await command(["cargo", ...args, "metadata", "--manifest-path", join(repository, "Cargo.toml"), "--format-version", "1", "--filter-platform", "aarch64-apple-darwin"]);
const metadata = JSON.parse(metadataResult.stdout);
const surface = audit(metadata, ["chuzz"]);
const reportDirectory = join(repository, "target", profile, "sandbox2");
await mkdir(reportDirectory, { recursive: true });
await Bun.write(join(reportDirectory, "metadata.json"), metadataResult.stdout);
await Bun.write(join(reportDirectory, "c-surface.json"), JSON.stringify(surface, null, 2));
if (!surface.passed) throw new Error("c-surface-forbidden: " + surface.violations.map(pkg => pkg.name).join(", "));

const build = Bun.spawn(["cargo", ...args, "build", "-p", "chuzz", "--bin", "chuzz-gui", "--bin", "chuzz-headless", ...(profile === "release" ? ["--release"] : [])], { cwd: repository, stdin: "inherit", stdout: "inherit", stderr: "inherit" });
if (await build.exited !== 0) throw new Error("browser build failed");
await rm(bundle, { recursive: true, force: true });
await mkdir(macos, { recursive: true });
await mkdir(resources, { recursive: true });
await cp(join(script, "Info.plist"), join(contents, "Info.plist"));
await cp(join(script, "icons/icon.icns"), join(resources, "icon.icns"));
// The BSD notice accompanies the binary distribution, not only its source.
await cp(join(script, "src/sandbox/chuzz.sb"), join(resources, "chuzz.sb"));
for (const key of ["CFBundleShortVersionString", "CFBundleVersion"]) {
  await command(["/usr/libexec/PlistBuddy", "-c", `Set :${key} ${version}`, join(contents, "Info.plist")]);
}
for (const name of ["chuzz-headless", "chuzz-gui"]) {
  const binary = join(macos, name);
  await cp(join(repository, "target", profile, name), binary);
  await command(["/usr/bin/codesign", "--force", "--sign", "-", "--options", "runtime", "--entitlements", join(script, "entitlements.plist"), binary]);
  await verifySignature(binary);
}
await command(["/usr/bin/codesign", "--force", "--sign", "-", "--options", "runtime", "--entitlements", join(script, "entitlements.plist"), bundle]);
await command(["/usr/bin/codesign", "--verify", "--deep", "--strict", bundle]);
await verifySignature(join(macos, "chuzz-gui"));
await command([process.execPath, join(repository, "scripts/qa-sandbox2.ts"), "--startup-only", "--bundle", bundle]);
const now = new Date();
await utimes(bundle, now, now);
console.log(bundle);
