import { Buffer } from "node:buffer";

export type Notification = { sequence: number; value: Record<string, unknown> };

export class Rpc {
  private socket?: Awaited<ReturnType<typeof Bun.connect>>;
  private input = Buffer.alloc(0);
  private next = 1;
  private sequence = 0;
  private pending = new Map<number, { resolve: (value: any) => void; reject: (error: Error) => void }>();
  private events: Notification[] = [];
  private readers = new Set<() => void>();

  static async open(unix: string): Promise<Rpc> {
    const rpc = new Rpc();
    rpc.socket = await Bun.connect({ unix, socket: {
      data: (_, bytes) => rpc.receive(bytes),
      error: (_, error) => rpc.fail(error),
      close: () => rpc.fail(new Error("control socket closed")),
    }});
    await rpc.request("initialize", { protocolVersion: "2024-11-05", capabilities: {}, clientInfo: { name: "tab-lifecycle-witness", version: "1" } });
    rpc.send({ jsonrpc: "2.0", method: "notifications/initialized" });
    await rpc.tool("blitz.diagnostics", { allTargets: true, afterSequence: 0, command: "observe", params: { streams: ["console", "paint"] } });
    return rpc;
  }

  private fail(error: Error): void {
    for (const waiter of this.pending.values()) { waiter.reject(error); }
    this.pending.clear();
    for (const reader of this.readers) { reader(); }
  }

  private receive(bytes: Uint8Array): void {
    this.input = Buffer.concat([this.input, bytes]);
    for (;;) {
      if (this.input.length < 4) return;
      const length = this.input.readUInt32BE(0);
      if (length < 1 || length > 16 * 1024 * 1024) { this.fail(new Error("invalid control frame")); this.close(); return; }
      if (this.input.length < 4 + length) return;
      const frame = this.input.subarray(4, 4 + length);
      this.input = this.input.subarray(4 + length);
      if (frame[0] !== 0) { this.fail(new Error("non-text control frame")); return; }
      const value = JSON.parse(frame.subarray(1).toString());
      if (typeof value.id === "number") {
        const waiter = this.pending.get(value.id);
        if (!waiter) continue;
        this.pending.delete(value.id);
        if (value.error) waiter.reject(new Error(JSON.stringify(value.error)));
        else waiter.resolve(value.result);
      } else {
        this.events.push({ sequence: ++this.sequence, value });
        if (this.events.length > 256) this.events.shift();
        for (const reader of this.readers) reader();
      }
    }
  }

  private send(value: unknown): void {
    const body = Buffer.from(JSON.stringify(value));
    const frame = Buffer.alloc(body.length + 5);
    frame.writeUInt32BE(body.length + 1);
    frame[4] = 0;
    body.copy(frame, 5);
    this.socket!.write(frame);
  }

  async request(method: string, params: unknown): Promise<any> {
    const id = this.next++;
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      return await new Promise((resolve, reject) => {
        timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`control request ${method} timed out`)); }, 15_000);
        this.pending.set(id, { resolve, reject });
        this.send({ jsonrpc: "2.0", id, method, params });
      });
    } finally { clearTimeout(timer); }
  }

  async tool(name: string, arguments_: unknown): Promise<any> {
    const result = await this.request("tools/call", { name, arguments: arguments_ });
    const response = result?.structuredContent;
    if (!response || response.result === "error") throw new Error(`control refused ${name}: ${JSON.stringify(response)}`);
    return response;
  }

  mark(): number { return this.sequence; }

  async notice(after: number, marker: string, deadline: number): Promise<number> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      return await new Promise((resolve, reject) => {
        const read = () => {
          const event = this.events.find(event => event.sequence > after && JSON.stringify(event.value).includes(marker));
          if (event) { this.readers.delete(read); resolve(event.sequence); }
          else if (Date.now() >= deadline) { this.readers.delete(read); reject(new Error(`notice deadline: ${marker}`)); }
        };
        this.readers.add(read);
        timer = setTimeout(read, Math.max(0, deadline - Date.now()));
        read();
      });
    } finally { clearTimeout(timer); }
  }

  async evaluate(script: string, tabId?: number): Promise<any> {
    const response = await this.tool("blitz.diagnostics", { target: tabId === undefined ? "chrome" : { tabId }, command: "evaluate", params: { script } });
    if (response.result !== "evaluated") throw new Error("evaluation has no completion value");
    return response.value;
  }

  async invoke(command: string, args: unknown = {}): Promise<any> {
    const marker = `QA_COMMAND_${this.next}`;
    const after = this.mark();
    await this.evaluate(`globalThis.__qaReply = null; __IZUMO__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)}).then(value => { globalThis.__qaReply = {value}; console.log(${JSON.stringify(marker)}); }, error => { globalThis.__qaReply = {error: String(error)}; console.log(${JSON.stringify(marker)}); }); null`);
    await this.notice(after, marker, Date.now() + 15_000);
    const reply = await this.evaluate("globalThis.__qaReply");
    if (reply.error) throw new Error(reply.error);
    return reply.value;
  }

  async subscribeTabs(): Promise<void> {
    const after = this.mark();
    await this.evaluate(`globalThis.__qaTabs = []; __IZUMO__.listen('tabs-changed', event => { globalThis.__qaTabs = event.payload; console.log('QA_TABS_UPDATE'); }).then(() => __IZUMO__.invoke('list_tabs')).then(tabs => { globalThis.__qaTabs = tabs; console.log('QA_TABS_READY'); }); null`);
    await this.notice(after, "QA_TABS_READY", Date.now() + 15_000);
  }

  async tabsUntil(predicate: (tabs: any[]) => boolean, milliseconds: number): Promise<any[]> {
    const deadline = Date.now() + milliseconds;
    let after = this.mark();
    let tabs = await this.evaluate("globalThis.__qaTabs");
    while (!predicate(tabs)) {
      try { after = await this.notice(after, "QA_TABS_UPDATE", deadline); }
      catch { return await this.evaluate("globalThis.__qaTabs"); }
      tabs = await this.evaluate("globalThis.__qaTabs");
    }
    return tabs;
  }

  async act(tabId: number, domId: string, action: "click" | "setValue", value?: string): Promise<void> {
    const snapshot = await this.tool("blitz.agent.control", { target: { tabId }, command: "inspect", params: { root: null, maxDepth: 40 } });
    const node = snapshot.value.nodes.find((node: any) => node.domId === domId);
    if (!node) throw new Error(`preparation lacks #${domId}`);
    await this.tool("blitz.agent.control", { target: { tabId }, expectedGeneration: snapshot.context.generation,
      expectedDocumentId: snapshot.context.documentId, command: "act", params: { action, params: { nodeId: node.id, ...(value === undefined ? {} : { value }) } } });
  }

  close(): void { this.socket?.end(); }
}
