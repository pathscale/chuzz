import net from "node:net";

const socketPath = process.argv[2];
if (!socketPath) throw new Error("usage: bun scripts/smoke-control.ts <socket>");

const initialize = {
  jsonrpc: "2.0",
  id: 1,
  method: "initialize",
  params: {
    protocolVersion: "2025-06-18",
    capabilities: {},
    clientInfo: { name: "chuzz-smoke", version: "1" },
  },
};
const tool = (id: number, command: string, params: Record<string, unknown>) => ({
  jsonrpc: "2.0",
  id,
  method: "tools/call",
  params: { name: "blitz.agent.control", arguments: { command, params } },
});
const inspect = (id: number) => tool(id, "inspect", { root: null, max_depth: 40 });
const key = (id: number, phase: "down" | "up") =>
  tool(id, "act", {
    action: "input",
    params: {
      input: "key",
      phase,
      key: "t",
      code: "KeyT",
      modifiers: { shift: false, control: false, alt: false, meta: true },
    },
  });

function frame(value: unknown): Buffer {
  const payload = Buffer.from(JSON.stringify(value));
  const header = Buffer.alloc(5);
  header.writeUInt32BE(payload.length + 1);
  header[4] = 0;
  return Buffer.concat([header, payload]);
}

/** What the control plane answers with. Only the part this script reads. */
type ControlResponse = {
  id?: number;
  result?: { structuredContent?: { value?: { nodes?: Record<string, unknown>[] } } };
};

function semanticNodeCount(response: ControlResponse): number {
  const nodes = response.result?.structuredContent?.value?.nodes ?? [];
  return nodes.length;
}

function addressNode(response: ControlResponse): Record<string, unknown> | undefined {
  const nodes = response.result?.structuredContent?.value?.nodes ?? [];
  return nodes.find((node: Record<string, unknown>) =>
    JSON.stringify(node).includes("chuzz-address-bar"),
  );
}

const socket = net.createConnection(socketPath);
let buffer = Buffer.alloc(0);
let before = -1;
let initialAddress: Record<string, unknown> | undefined;
const send = (value: unknown) => socket.write(frame(value));

socket.on("connect", () => send(initialize));
socket.on("data", (data) => {
  buffer = Buffer.concat([buffer, data]);
  while (buffer.length >= 4) {
    const length = buffer.readUInt32BE();
    if (buffer.length < length + 4) return;
    const response: ControlResponse = JSON.parse(buffer.subarray(5, 4 + length).toString());
    buffer = buffer.subarray(4 + length);
    switch (response.id) {
      case 1:
        send(inspect(2));
        break;
      case 2:
        before = semanticNodeCount(response);
        initialAddress = addressNode(response);
        if (process.env.CHUZZ_SMOKE_DUMP === "1") {
          const nodes = response.result?.structuredContent?.value?.nodes ?? [];
          console.error(
            JSON.stringify(
              nodes.filter(
                (node: Record<string, unknown>) =>
                  node.role === "textbox" || node.role === "button" || node.role === "status",
              ),
            ),
          );
        }
        send(key(3, "down"));
        break;
      case 3:
        send(key(4, "up"));
        break;
      case 4:
        setTimeout(() => send(inspect(5)), 700);
        break;
      case 5: {
        const after = semanticNodeCount(response);
        console.log(JSON.stringify({ before, after, initialAddress }));
        socket.end();
        process.exit(after > before ? 0 : 3);
      }
    }
  }
});

setTimeout(() => {
  console.error("control smoke test timed out");
  socket.destroy();
  process.exit(2);
}, 10_000);
