import { dlopen, FFIType, ptr } from "bun:ffi";
import { readdirSync, readFileSync } from "node:fs";

type Limits = Record<string, number>;
type Child = { kill(signal?: number | NodeJS.Signals): unknown; exited: Promise<number> };
export function installCaps(name: string, defaults: Limits = {}) {
  const names = ["rss", "cpu-secs", "wall-secs", "open-files", "procs", "threads", "input-bytes", "iterations", "cache-entries", "decoded-bytes"];
  const hard: Limits = {}, warn: Limits = {};
  const number = (text: string): number => {
    const match = /^(\d+)([KMG]?)$/i.exec(text);
    const scale: Record<string, number> = { "": 1, K: 1024, M: 1024 ** 2, G: 1024 ** 3 };
    const value = match ? Number(match[1]) * scale[match[2].toUpperCase()] : NaN;
    if (!Number.isSafeInteger(value) || value <= 0) throw new Error(`CAP_BAD_PARAMETER ${text}`);
    return value;
  };
  if (process.env.APP_CAP_PROFILE && process.env.APP_CAP_PROFILE !== "default") throw new Error("CAP_BAD_PARAMETER APP_CAP_PROFILE");
  for (const field of names) {
    const key = field.replaceAll("-", "_").toUpperCase();
    const maximum = process.env[`APP_CAP_MAX_${key}`], warning = process.env[`APP_CAP_WARN_${key}`];
    if (maximum !== undefined) hard[field] = number(maximum);
    else if (process.env.APP_CAP_PROFILE === "default" && defaults[field] !== undefined) hard[field] = defaults[field];
    else if (warning !== undefined) throw new Error(`CAP_BAD_PARAMETER warn-${field} needs max-${field}`);
    if (hard[field] !== undefined) {
      warn[field] = warning === undefined ? Math.max(1, Math.floor(hard[field] / 2)) : number(warning);
      if (warn[field] > hard[field]) throw new Error(`CAP_BAD_PARAMETER warn-${field} exceeds max-${field}`);
    }
  }
  let label = "startup", turns = 0;
  const started = performance.now(), warned = new Set<string>(), owned = new Set<Child>();
  function stop(field: string, value: number): never {
    console.error(`CAP_${field.replaceAll("-", "_").toUpperCase()}_EXCEEDED app=${name} current=${value} threshold=${hard[field]} stage=${JSON.stringify(label)}`);
    for (const child of owned) { try { child.kill("SIGKILL"); } catch {} }
    process.exit(78);
  }
  function check(field: string, value: number): void {
    if (hard[field] === undefined) return;
    if (value >= warn[field] && !warned.has(field)) {
      warned.add(field);
      console.error(`CAP_WARN_${field.replaceAll("-", "_").toUpperCase()} app=${name} current=${value} threshold=${warn[field]} stage=${JSON.stringify(label)}`);
    }
    if (value > hard[field]) stop(field, value);
  }
  let native: ReturnType<typeof dlopen> | undefined;
  if (process.platform === "darwin" && ["open-files", "procs", "threads"].some(key => hard[key] !== undefined)) {
    native = dlopen("/usr/lib/libSystem.B.dylib", {
      proc_pidinfo: { args: [FFIType.i32, FFIType.i32, FFIType.u64, FFIType.ptr, FFIType.i32], returns: FFIType.i32 },
      proc_listchildpids: { args: [FFIType.i32, FFIType.ptr, FFIType.i32], returns: FFIType.i32 },
    });
  }
  function census(): void {
    if (hard.rss !== undefined) check("rss", process.memoryUsage().rss);
    if (hard["cpu-secs"] !== undefined) { const usage = process.resourceUsage(); check("cpu-secs", (usage.userCPUTime + usage.systemCPUTime) / 1e6); }
    check("wall-secs", (performance.now() - started) / 1000);
    if (!["open-files", "procs", "threads"].some(key => hard[key] !== undefined)) return;
    if (process.platform === "linux") {
      if (hard["open-files"] !== undefined) check("open-files", Math.max(0, readdirSync("/proc/self/fd").length - 1));
      if (hard.threads !== undefined) {
        const match = /^Threads:\s*(\d+)/m.exec(readFileSync("/proc/self/status", "utf8"));
        if (!match) throw new Error("CAP_METRICS_UNAVAILABLE threads");
        check("threads", Number(match[1]));
      }
      if (hard.procs !== undefined) {
        const children = new Set<string>();
        for (const task of readdirSync("/proc/self/task")) {
          for (const pid of readFileSync(`/proc/self/task/${task}/children`, "utf8").split(/\s+/).filter(Boolean)) children.add(pid);
        }
        check("procs", children.size);
      }
    } else if (native) {
      const task = new Uint8Array(96);
      if (Number(native.symbols.proc_pidinfo(process.pid, 4, 0, ptr(task), task.byteLength)) !== task.byteLength) throw new Error("CAP_METRICS_UNAVAILABLE task");
      check("threads", new DataView(task.buffer).getInt32(84, true));
      if (hard["open-files"] !== undefined) {
        const slots = new Uint8Array((Math.min(hard["open-files"], 65536) + 1) * 8);
        const count = Number(native.symbols.proc_pidinfo(process.pid, 1, 0, ptr(slots), slots.byteLength));
        if (count < 0) throw new Error("CAP_METRICS_UNAVAILABLE descriptors");
        check("open-files", count / 8);
        if (count >= slots.byteLength) throw new Error("CAP_METRICS_UNAVAILABLE descriptor census overflow");
      }
      if (hard.procs !== undefined) {
        const slots = new Int32Array(4096);
        const count = Number(native.symbols.proc_listchildpids(process.pid, ptr(slots), slots.byteLength));
        if (count < 0) throw new Error("CAP_METRICS_UNAVAILABLE children");
        check("procs", slots.subarray(0, count / 4).filter(pid => pid > 0).length);
        if (count >= slots.byteLength) throw new Error("CAP_METRICS_UNAVAILABLE child census overflow");
      }
    } else throw new Error("CAP_PLATFORM_UNSUPPORTED selected process census");
  }
  const reserve = (size: number): void => { census(); if (hard.rss !== undefined) check("rss", process.memoryUsage().rss + size); };
  if (hard["wall-secs"] !== undefined) {
    const alarm = (threshold: number): void => {
      const remaining = threshold * 1000 - (performance.now() - started);
      setTimeout(() => {
        const elapsed = (performance.now() - started) / 1000;
        if (elapsed >= threshold) check("wall-secs", elapsed); else alarm(threshold);
      }, Math.max(0, Math.min(2_147_483_647, remaining))).unref();
    };
    alarm(warn["wall-secs"]);
    alarm(hard["wall-secs"] + 0.001);
  }
  census();
  return {
    check, reserve, census,
    limit(field: string): number { return hard[field] ?? Number.MAX_SAFE_INTEGER; },
    stage(value: string): void { label = value; census(); },
    turn(value: string): void { label = value; census(); check("iterations", ++turns); },
    beforeChild(): void { census(); check("procs", owned.size + 1); },
    watch(child: Child): void { owned.add(child); void child.exited.then(() => owned.delete(child), () => owned.delete(child)); },
    async bytes(stream: ReadableStream<Uint8Array>, value = "read stream", maximum = this.limit("input-bytes")): Promise<Uint8Array> {
      label = value;
      const reader = stream.getReader(), chunks: Uint8Array[] = [];
      let size = 0;
      try {
        for (;;) {
          const chunk = await reader.read();
          if (chunk.done) break;
          census();
          const next = size + chunk.value.byteLength;
          check("input-bytes", next);
          if (next > maximum) throw new Error(`CAP_INPUT_BYTES_EXCEEDED current=${next} threshold=${maximum} stage=${value}`);
          check("cache-entries", chunks.length + 1);
          chunks.push(chunk.value); size = next;
        }
        reserve(size);
        check("decoded-bytes", size * 2);
        const result = new Uint8Array(size);
        let offset = 0;
        for (const chunk of chunks) { result.set(chunk, offset); offset += chunk.byteLength; }
        return result;
      } finally { await reader.cancel(); }
    },
    async fileBytes(path: string): Promise<Uint8Array> {
      const file = Bun.file(path); check("input-bytes", file.size);
      return this.bytes(file.stream(), `read ${path}`);
    },
    async text(path: string): Promise<string> {
      const bytes = await this.fileBytes(path); reserve(bytes.byteLength * 2);
      return new TextDecoder().decode(bytes);
    },
    async streamText(stream: ReadableStream<Uint8Array>): Promise<string> {
      const bytes = await this.bytes(stream); reserve(bytes.byteLength * 2);
      return new TextDecoder().decode(bytes);
    },
  };
}
