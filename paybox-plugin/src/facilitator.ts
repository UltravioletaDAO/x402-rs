// The one place this plugin uses the network: the facilitator's MCP, POST /mcp on the facilitator's
// one host (the only network host configured in PayBox for this plugin). Each tools/call goes there by
// the same name, with the same arguments, and the facilitator does the rest: it validates them, applies
// its limits, runs the read through its own REST route and answers. This module adds nothing to that
// answer; it only bounds the wait and the size, and says what happened when there is no answer to pass on.
import type { CallResult } from "./catalogo.js";

/** The facilitator's one host. */
export const ORIGIN = "https://facilitator.ultravioletadao.xyz";

/**
 * Of PayBox's 10 s per operation, the sandbox takes about 4 (measured by the other plugins of the
 * stack): the facilitator gets the 6 left. Its reads answer from a cache or from one view call.
 */
export const TIMEOUT_MS = 6_000;

/**
 * The most of the facilitator's answer this plugin reads: one result must fit PayBox's 256 KiB of
 * stdout. The largest read, x402_supported, weighed 94,482 bytes over POST /mcp on 2026-11-02.
 */
export const MAX_ANSWER_BYTES = 192 * 1024;

/** The MCP revision the facilitator's streamable HTTP endpoint speaks, without a session. */
export const FACILITATOR_PROTOCOL_VERSION = "2025-06-18";

export type FetchLike = (input: string, init: RequestInit) => Promise<Response>;

export type Failure = "timeout" | "network" | "http" | "too_large" | "unreadable";

export type Answer =
  | { kind: "result"; result: CallResult }
  | { kind: "error"; code: number; message: string; data?: unknown }
  | { kind: "failure"; failure: Failure; status?: number };

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

/** The URL of `path` on the facilitator's host, and never on another one. */
export function urlOf(path: string): string {
  const url = new URL(path, ORIGIN);
  if (url.origin !== ORIGIN || !path.startsWith("/") || path.startsWith("//")) {
    throw new Error(`not a path on ${ORIGIN}: ${path}`);
  }
  return url.toString();
}

/** One tools/call to the facilitator: one request, no retry, no redirect, bounded in time and size. */
export async function callFacilitator(
  fetchImpl: FetchLike,
  path: string,
  name: string,
  args: Record<string, unknown>,
  userAgent: string,
): Promise<Answer> {
  let response: Response;
  try {
    response = await fetchImpl(urlOf(path), {
      method: "POST",
      headers: {
        "content-type": "application/json",
        accept: "application/json, text/event-stream",
        "mcp-protocol-version": FACILITATOR_PROTOCOL_VERSION,
        "user-agent": userAgent,
      },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name, arguments: args } }),
      redirect: "error",
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  } catch (error) {
    return { kind: "failure", failure: isTimeout(error) ? "timeout" : "network" };
  }
  if (response.status !== 200) {
    await response.body?.cancel().catch(() => undefined);
    return { kind: "failure", failure: "http", status: response.status };
  }
  let text: string | null;
  try {
    text = await readCapped(response, MAX_ANSWER_BYTES);
  } catch (error) {
    return { kind: "failure", failure: isTimeout(error) ? "timeout" : "unreadable" };
  }
  if (text === null) return { kind: "failure", failure: "too_large" };
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    return { kind: "failure", failure: "unreadable" };
  }
  if (!isRecord(body) || body.jsonrpc !== "2.0") return { kind: "failure", failure: "unreadable" };
  const { result, error } = body;
  if (isRecord(result) && Array.isArray(result.content)) {
    return { kind: "result", result: resultOf(result) };
  }
  if (isRecord(error) && typeof error.code === "number" && typeof error.message === "string") {
    return { kind: "error", code: error.code, message: error.message, ...("data" in error ? { data: error.data } : {}) };
  }
  return { kind: "failure", failure: "unreadable" };
}

/** The facilitator's result with the four fields a tools/call result carries, and nothing else. */
function resultOf(result: Record<string, unknown>): CallResult {
  return {
    content: result.content as unknown[],
    ...(isRecord(result.structuredContent) ? { structuredContent: result.structuredContent } : {}),
    ...(typeof result.isError === "boolean" ? { isError: result.isError } : {}),
    ...(isRecord(result._meta) ? { _meta: result._meta } : {}),
  };
}

function isTimeout(error: unknown): boolean {
  return error instanceof DOMException && (error.name === "TimeoutError" || error.name === "AbortError");
}

/** The body as UTF-8 text, or null when it is longer than `max` bytes (reading stops there). */
export async function readCapped(response: Response, max: number): Promise<string | null> {
  if (!response.body) return "";
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > max) {
      await reader.cancel().catch(() => undefined);
      return null;
    }
    chunks.push(value);
  }
  return new TextDecoder("utf-8", { fatal: true }).decode(Buffer.concat(chunks));
}
