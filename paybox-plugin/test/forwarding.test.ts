// tools/call goes to the facilitator, on its one host, by the same name and with the same arguments:
// one request, no retry, no redirect, bounded in time (6 s) and size (192 KiB). The facilitator's result
// comes back as it came (content, structuredContent, isError, _meta); when there is no result to pass
// on, the agent reads what happened.
import { test } from "node:test";
import assert from "node:assert/strict";

import { CATALOG } from "../src/catalogo.ts";
import { callFacilitator, MAX_ANSWER_BYTES, ORIGIN, TIMEOUT_MS, urlOf } from "../src/facilitator.ts";
import { USER_AGENT } from "../src/server.ts";
import { callTool, deps, facilitatorResult, fakeFetch, json, rpc, rpcLine, text } from "./harness.ts";

const TOOL = "x402_chain_health";

test("one POST to the facilitator's MCP, on its one host, with the same name and arguments", async () => {
  const answer = { content: [{ type: "text", text: "{\"status\":\"ok\"}" }], structuredContent: { status: "ok", networks: [] }, _meta: { "uvd/eco": 1 } };
  const { fetchImpl, calls } = fakeFetch(facilitatorResult(answer));
  const result = await callTool(deps(fetchImpl), TOOL, { network: "base" });
  assert.equal(calls.length, 1);
  const [call] = calls;
  assert.equal(call.url, `${ORIGIN}${CATALOG.ruta}`);
  assert.equal(call.url, "https://facilitator.ultravioletadao.xyz/mcp");
  assert.equal(call.method, "POST");
  assert.equal(call.redirect, "error");
  assert.ok(call.signal, "the call is bounded in time");
  assert.deepEqual(JSON.parse(call.body ?? ""), {
    jsonrpc: "2.0",
    id: 1,
    method: "tools/call",
    params: { name: TOOL, arguments: { network: "base" } },
  });
  assert.deepEqual(call.headers, {
    "content-type": "application/json",
    accept: "application/json, text/event-stream",
    "mcp-protocol-version": "2025-06-18",
    "user-agent": USER_AGENT,
  });
  assert.deepEqual(result.content, answer.content);
  assert.deepEqual(result.structuredContent, answer.structuredContent);
  // The facilitator's _meta as it came (the SDK adds its own serverInfo next to it).
  assert.deepEqual(result._meta["uvd/eco"], 1);
  assert.notEqual(result.isError, true);
});

test("every listed tool goes out by its own name, and only by it", async () => {
  for (const name of CATALOG.tools.map((t) => t.name)) {
    const { fetchImpl, calls } = fakeFetch(facilitatorResult({ content: [{ type: "text", text: "ok" }], isError: true }));
    await callTool(deps(fetchImpl), name, {});
    assert.equal(calls.length, 1, name);
    assert.equal(JSON.parse(calls[0].body ?? "").params.name, name);
  }
});

test("an isError result from the facilitator comes back as is", async () => {
  const answer = { content: [{ type: "text", text: "no DX402 evidence for this paymentId" }], isError: true };
  const { fetchImpl } = fakeFetch(facilitatorResult(answer));
  const result = await callTool(deps(fetchImpl), "x402_durable_evidence", { paymentId: "0xabc" });
  assert.equal(result.isError, true);
  assert.equal(text(result), answer.content[0].text);
});

test("a JSON-RPC error from the facilitator is an isError result with its message and data", async () => {
  const error = { code: -32602, message: "paymentId must be a non-empty string", data: { argument: "paymentId" } };
  const { fetchImpl } = fakeFetch(json(200, { jsonrpc: "2.0", id: 1, error }));
  const result = await callTool(deps(fetchImpl), "x402_durable_evidence", { paymentId: " " });
  assert.equal(result.isError, true);
  assert.match(text(result), /paymentId must be a non-empty string/);
  assert.match(text(result), /"argument":"paymentId"/);
  assert.deepEqual(result.structuredContent, { error });
});

test("the facilitator's list of its own tools never reaches the agent, and the rest of its error data does", async () => {
  // As a facilitator older than these reads answers one of them: its tools are every tool of its MCP.
  const error = { code: -32601, message: "unknown tool: x402_networks", data: { tools: ["x402_supported", "x402_accepts", "x402_verify", "x402_settle"] } };
  const { fetchImpl } = fakeFetch(json(200, { jsonrpc: "2.0", id: 1, error }));
  const result = await callTool(deps(fetchImpl), "x402_networks");
  assert.equal(result.isError, true);
  assert.doesNotMatch(JSON.stringify(result), /x402_settle|x402_verify|x402_accepts/);
  assert.deepEqual(result.structuredContent, { error: { code: -32601, message: "unknown tool: x402_networks" } });
  const mixed = { code: -32602, message: "invalid arguments", data: { tools: ["x"], argument: "network" } };
  const second = await callTool(deps(fakeFetch(json(200, { jsonrpc: "2.0", id: 1, error: mixed })).fetchImpl), TOOL);
  assert.deepEqual(second.structuredContent, { error: { code: -32602, message: "invalid arguments", data: { argument: "network" } } });
});

for (const [name, answer, says] of [
  ["a timeout", new DOMException("timed out", "TimeoutError"), `within ${TIMEOUT_MS / 1000} s`],
  ["no network", new TypeError("fetch failed"), "could not be reached"],
  ["HTTP 503", new Response("{}", { status: 503 }), "HTTP 503"],
  ["HTTP 429", new Response("{}", { status: 429 }), "limiting calls"],
  ["HTTP 406", new Response("{}", { status: 406 }), "HTTP 406"],
  ["a redirect", new Response("", { status: 302, headers: { location: "https://elsewhere.example/" } }), "HTTP 302"],
  ["not JSON", new Response("<html>", { status: 200 }), "could not be read"],
  ["JSON that is not JSON-RPC", json(200, { result: { content: [] } }), "could not be read"],
  ["a result without content", json(200, { jsonrpc: "2.0", id: 1, result: { structuredContent: {} } }), "could not be read"],
  ["an answer past 192 KiB", new Response("x".repeat(MAX_ANSWER_BYTES + 1), { status: 200 }), "larger than one result"],
] as const) {
  test(`${name}: an isError result that says so`, async () => {
    const { fetchImpl, calls } = fakeFetch(answer);
    const result = await callTool(deps(fetchImpl), TOOL);
    assert.equal(calls.length, 1, "one request, no retry");
    assert.equal(result.isError, true);
    assert.ok(text(result).includes(says), text(result));
  });
}

test("the facilitator gets at most the 6 s PayBox leaves", () => {
  assert.ok(TIMEOUT_MS <= 6_000, `${TIMEOUT_MS} ms`);
  assert.ok(TIMEOUT_MS >= 3_000, `${TIMEOUT_MS} ms`);
});

test("a body that is not UTF-8 is unreadable, never text with replacement characters", async () => {
  const valid = JSON.stringify({ jsonrpc: "2.0", id: 1, result: { content: [{ type: "text", text: "@" }] } });
  const [before, after] = valid.split("@");
  const body = (middle: number[]) => Buffer.concat([Buffer.from(before), Buffer.from(middle), Buffer.from(after)]);
  const call = (bytes: Buffer) => callFacilitator(fakeFetch(new Response(bytes, { status: 200 })).fetchImpl, CATALOG.ruta, TOOL, {}, USER_AGENT);
  // The same body with valid UTF-8 in the middle is read: only the bytes make it unreadable.
  assert.equal((await call(body([0xc3, 0xa9]))).kind, "result");
  assert.deepEqual(await call(body([0xff, 0xfe])), { kind: "failure", failure: "unreadable" });
  const result = await callTool(deps(fakeFetch(new Response(body([0xff, 0xfe]), { status: 200 })).fetchImpl), TOOL);
  assert.equal(result.isError, true);
  assert.ok(text(result).includes("could not be read"), text(result));
});

test("U+2028 and U+2029 in what the facilitator returns go out escaped, and the answer stays one line", async () => {
  const separators = [0x2028, 0x2029].map((code) => String.fromCharCode(code));
  const said = `before${separators.join("")}after`;
  const { fetchImpl } = fakeFetch(facilitatorResult({ content: [{ type: "text", text: said }] }));
  const line = await rpcLine(deps(fetchImpl), "tools/call", { name: TOOL, arguments: {} });
  for (const raw of separators) assert.ok(!line.includes(raw), `raw U+${raw.charCodeAt(0).toString(16)} on stdout`);
  for (const hex of ["2028", "2029"]) assert.ok(line.includes("\\" + "u" + hex), line);
  assert.equal(JSON.parse(line).result.content[0].text, said);
});

test("no answer: one more call later is safe, never a loop (as the instructions say)", async () => {
  for (const failure of [new DOMException("timed out", "TimeoutError"), new TypeError("fetch failed")]) {
    const result = await callTool(deps(fakeFetch(failure).fetchImpl), TOOL);
    assert.match(text(result), /one more call later is safe, never a loop\.$/);
  }
});

test("an answer of exactly 192 KiB is still read", async () => {
  const filler = "y".repeat(MAX_ANSWER_BYTES - 200);
  const body = JSON.stringify({ jsonrpc: "2.0", id: 1, result: { content: [{ type: "text", text: filler }] } });
  const padded = body.length < MAX_ANSWER_BYTES ? body + " ".repeat(MAX_ANSWER_BYTES - body.length) : body;
  assert.equal(Buffer.byteLength(padded), MAX_ANSWER_BYTES);
  const { fetchImpl } = fakeFetch(new Response(padded, { status: 200 }));
  const result = await callTool(deps(fetchImpl), TOOL);
  assert.notEqual(result.isError, true, text(result).slice(0, 80));
});

test("a tool this plugin does not list is a JSON-RPC error, and nothing goes out", async () => {
  const { fetchImpl, calls } = fakeFetch();
  for (const name of ["x402_settle", "x402_verify", "x402_accepts", "x402_register", "nope"]) {
    const response = await rpc(deps(fetchImpl), "tools/call", { name, arguments: {} });
    assert.equal(response.error?.code, -32602, JSON.stringify(response));
  }
  assert.equal(calls.length, 0);
});

test("the client only builds URLs on the facilitator's host", () => {
  assert.equal(urlOf("/mcp"), "https://facilitator.ultravioletadao.xyz/mcp");
  for (const bad of ["//elsewhere.example/mcp", "https://elsewhere.example/mcp", "mcp", "http://facilitator.ultravioletadao.xyz/mcp"]) {
    assert.throws(() => urlOf(bad), /not a path/, bad);
  }
});
