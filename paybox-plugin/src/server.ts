// The cascaron: an MCP server with no domain logic, no signing and no payments. tools/list is the
// generated catalog, the reads of the facilitator's own tools/list; tools/call goes to the facilitator
// by the same name (src/facilitator.ts). One capability, one implementation: the facilitator runs each
// read through its REST route, and this plugin relays it.
import { ProtocolError, Server } from "@modelcontextprotocol/server";
import type { CallToolResult, Tool } from "@modelcontextprotocol/server";

import { CATALOG, type CallResult, type ToolDefinition } from "./catalogo.js";
import { callFacilitator, TIMEOUT_MS, type Answer, type FetchLike } from "./facilitator.js";
import { INSTRUCTIONS } from "./instructions.js";

export const PLUGIN_NAME = "ultravioleta-facilitator";
export const PLUGIN_TITLE = "Ultravioleta DAO x402 Facilitator";
export const PLUGIN_VERSION = "0.1.0";
export const PROTOCOL_VERSION = "2026-07-28";
export const USER_AGENT = `uvd-facilitator-paybox-plugin/${PLUGIN_VERSION}`;

export interface Deps {
  fetch: FetchLike;
}

/** Every tool this plugin lists, by name, in the order tools/list returns them. */
export const TOOL_NAMES: readonly string[] = CATALOG.tools.map((t) => t.name);
const TOOLS: ReadonlyMap<string, ToolDefinition> = new Map(CATALOG.tools.map((t) => [t.name, t]));

export function createServer(deps: Deps): Server {
  const server = new Server(
    { name: PLUGIN_NAME, title: PLUGIN_TITLE, version: PLUGIN_VERSION },
    { capabilities: { tools: {} }, supportedProtocolVersions: [PROTOCOL_VERSION], instructions: INSTRUCTIONS },
  );
  server.setRequestHandler("tools/list", () => ({ tools: CATALOG.tools as unknown as Tool[] }));
  server.setRequestHandler("tools/call", async (request) => {
    const { name } = request.params;
    const tool = TOOLS.get(name);
    // A tool this plugin does not list never leaves it: the payment rails are not relayed.
    if (!tool) throw new ProtocolError(-32602, `Unknown tool: ${name}`, { valid_tools: TOOL_NAMES });
    const answer = await callFacilitator(deps.fetch, CATALOG.ruta, name, request.params.arguments ?? {}, USER_AGENT);
    return server.projectCallToolResult(asResult(answer) as CallToolResult, tool.outputSchema);
  });
  return server;
}

function text(message: string): CallResult {
  return { content: [{ type: "text", text: message }], isError: true };
}

/**
 * The facilitator's error data without `tools`: the facilitator lists there every tool of its MCP,
 * the payment rails included, which this plugin does not open; the agent has this plugin's tools/list.
 * What the data says about the arguments stays.
 */
export function errorData(data: unknown): unknown {
  if (data === null || typeof data !== "object" || Array.isArray(data)) return data;
  const { tools: _left_out, ...rest } = data as Record<string, unknown>;
  return Object.keys(rest).length === 0 ? undefined : rest;
}

/** What the agent reads: the facilitator's result as it came, or what happened when there was none. */
export function asResult(answer: Answer): CallResult {
  switch (answer.kind) {
    case "result":
      return answer.result;
    case "error": {
      const data = errorData(answer.data);
      return {
        content: [
          { type: "text", text: `The facilitator did not run the call: ${answer.message}` + (data === undefined ? "" : ` ${JSON.stringify(data)}`) },
        ],
        structuredContent: { error: { code: answer.code, message: answer.message, ...(data === undefined ? {} : { data }) } },
        isError: true,
      };
    }
    case "failure":
      switch (answer.failure) {
        case "timeout":
          return text(`The facilitator did not answer within ${TIMEOUT_MS / 1000} s. These tools only read: one more call later is safe, never a loop.`);
        case "network":
          return text("The facilitator could not be reached. These tools only read: one more call later is safe, never a loop.");
        case "http":
          return answer.status === 429
            ? text("The facilitator is limiting calls right now (HTTP 429): wait a minute before calling again.")
            : text(`The facilitator answered HTTP ${answer.status ?? "?"} instead of a result.`);
        case "too_large":
          return text("The facilitator's answer is larger than one result can carry: narrow the read (a network, one paymentId).");
        case "unreadable":
          return text("The facilitator's answer could not be read.");
      }
  }
}
