import net from "node:net";

const socketPath = process.argv[2];
if (!socketPath) throw new Error("usage: bun scripts/control-e2e.ts <socket>");

const request = (id: number, method: string, params: Record<string, unknown>) => ({
  jsonrpc: "2.0",
  id,
  method,
  params,
});
const tool = (id: number, command: string, params: Record<string, unknown>) =>
  request(id, "tools/call", {
    name: "blitz.agent.control",
    arguments: { command, params },
  });
const inspect = (id: number) => tool(id, "inspect", { root: null, max_depth: 40 });
const act = (id: number, action: string, params: Record<string, unknown>) =>
  tool(id, "act", { action, params });
const key = (id: number, phase: "down" | "up") =>
  act(id, "input", {
    input: "key",
    phase,
    key: "Enter",
    code: "Enter",
    modifiers: { shift: false, control: false, alt: false, meta: false },
  });

function frame(value: unknown): Buffer {
  const payload = Buffer.from(JSON.stringify(value));
  const header = Buffer.alloc(5);
  header.writeUInt32BE(payload.length + 1);
  header[4] = 0;
  return Buffer.concat([header, payload]);
}

type Node = {
  id: number;
  name: string;
  role: string;
  value: string | null;
  visible: boolean;
  bounds: [number, number, number, number];
};

/** What the control plane answers with. Only the part these scripts read. */
type ControlResponse = {
  id?: number;
  result?: { structuredContent?: { value?: { nodes?: Node[] } } };
};

const nodes = (response: ControlResponse): Node[] =>
  response.result?.structuredContent?.value?.nodes ?? [];

/**
 * The inspector handle, by the labels it actually carries.
 *
 * It names its direction rather than its target now, so matching one fixed
 * string finds it in only one of its two states. A script that silently fails
 * to find a control reports "the control did nothing", which is the wrong bug
 * to go looking for.
 */
const isPanelHandle = (node: Node) =>
  node.visible && (node.name === "Show inspector" || node.name === "Hide inspector");

const socket = net.createConnection(socketPath);
let buffer = Buffer.alloc(0);
let addressId = 0;
let panelId = 0;
let loadingSeen = false;
let initialVisible = 0;
let typedAfterSet: string | null | undefined;
let setResult: unknown;
let initialAddress: string | null | undefined;
let pageResult: { address: string | null | undefined; loaded: boolean } = {
  address: undefined,
  loaded: false,
};
const send = (value: unknown) => socket.write(frame(value));

socket.on("connect", () =>
  send(
    request(1, "initialize", {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "chuzz-control-e2e", version: "1" },
    }),
  ),
);

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
      case 2: {
        const tree = nodes(response);
        addressId = tree.find((node) => node.visible && node.role === "textbox")?.id ?? 0;
        initialAddress = tree.find((node) => node.visible && node.role === "textbox")?.value;
        panelId = tree.find(isPanelHandle)?.id ?? 0;
        initialVisible = tree.filter((node) => node.visible).length;
        if (!addressId || !panelId) throw new Error("address or panel control missing");
        send(act(3, "setValue", { nodeId: addressId, value: "https://example.com" }));
        break;
      }
      case 3:
        setResult = response.result?.structuredContent?.value ?? response.result;
        send(inspect(30));
        break;
      case 30:
        typedAfterSet = nodes(response).find(
          (node) => node.visible && node.role === "textbox",
        )?.value;
        send(key(4, "down"));
        break;
      case 4:
        send(key(5, "up"));
        break;
      case 5:
        setTimeout(() => send(inspect(6)), 100);
        break;
      case 6: {
        const tree = nodes(response);
        loadingSeen = tree.some(
          (node) => node.visible && node.name.toLowerCase().includes("loading"),
        );
        setTimeout(() => send(inspect(7)), 2500);
        break;
      }
      case 7: {
        const tree = nodes(response);
        const address = tree.find((node) => node.visible && node.role === "textbox")?.value;
        const loaded = tree.some((node) => node.visible && node.name.includes("Example Domain"));
        panelId = tree.find(isPanelHandle)?.id ?? 0;
        send(act(8, "click", { nodeId: panelId }));
        setTimeout(() => send(inspect(9)), 200);
        pageResult = { address, loaded };
        break;
      }
      case 8:
        break;
      case 9: {
        const tree = nodes(response);
        const result = pageResult;
        const panelChanged = tree.filter((node) => node.visible).length !== initialVisible;
        console.log(
          JSON.stringify({
            initialAddress,
            setResult,
            typedAfterSet,
            loadingSeen,
            panelChanged,
            ...result,
          }),
        );
        socket.end();
        process.exit(result.loaded && panelChanged ? 0 : 3);
      }
    }
  }
});

setTimeout(() => {
  console.error("control e2e timed out");
  socket.destroy();
  process.exit(2);
}, 10_000);
