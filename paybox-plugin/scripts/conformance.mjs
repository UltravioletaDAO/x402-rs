import { createHash } from "node:crypto";
import { readFile, rm, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { realpathSync } from "node:fs";
import { createInterface } from "node:readline";

import { checkEnvironment, ConformanceError, release, reportConformance } from "./report.mjs";
import { withProcess } from "./process.mjs";
import { compileSchema, validateSample } from "./schema.mjs";

export { ConformanceError, reportConformance };
const PROTOCOL_VERSION = release.protocolVersion;
const MAX_ARTIFACT_BYTES = 10 * 1024 * 1024;
const MAX_STDOUT_BYTES = 256 * 1024;
const MAX_STDERR_BYTES = 16 * 1024;
const MAX_TOOLS = 256;
const MAX_TOOL_SCHEMA_BYTES = 64 * 1024;
const MAX_CONTENT_ITEMS = 128;
const MAX_TOOL_NAME_BYTES = 128;
const MAX_ARGUMENT_BYTES = 64 * 1024;
const MAX_CURSOR_BYTES = 1024;
const OPERATION_TIMEOUT_MS = 10_000;

export async function testArtifact(root = process.cwd()) {
  const artifact = resolve(root, "dist/plugin.mjs");
  checkEnvironment();
  const bytes = await validateArtifact(artifact);
  let config;
  try {
    config = JSON.parse(await readFile(resolve(root, "paybox-plugin.json"), "utf8"));
  } catch {
    fail("Read a valid paybox-plugin.json with tool and arguments", "config_invalid");
  }
  object(config, "sample configuration", "config_invalid");
  if (typeof config.tool !== "string" || config.tool.length === 0) fail("Set the sample tool name", "config_invalid");
  object(config.arguments, "sample arguments", "config_invalid");
  if (Buffer.byteLength(config.tool) > MAX_TOOL_NAME_BYTES) fail(`sample tool name exceeds ${MAX_TOOL_NAME_BYTES} bytes`, "limit_exceeded");
  if (Buffer.byteLength(JSON.stringify(config.arguments)) > MAX_ARGUMENT_BYTES) fail(`sample arguments exceed ${MAX_ARGUMENT_BYTES} bytes`, "limit_exceeded");
  const tools = await listTools(artifact);
  const selected = tools.find((tool) => tool.name === config.tool);
  if (!selected) fail("sample tool was not returned by tools/list", "config_invalid");
  validateSample(selected, config.arguments);
  const response = await runOperation(artifact, {
    method: "tools/call",
    params: { name: config.tool, arguments: config.arguments },
  });
  validateCallResult(response.result);
  if (response.result.isError === true) fail("sample tools/call returned isError: true", "tool_failed");
  validateSample(selected, config.arguments, response.result);
  return {
    status: "passed",
    sampleCall: { tool: config.tool, status: "passed" },
    artifact: { bytes: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") },
    discoveredToolCount: tools.length,
  };
}

async function validateArtifact(artifact) {
  if (basename(artifact) !== "plugin.mjs") fail("artifact must be named plugin.mjs", "artifact_invalid");
  let bytes;
  try {
    bytes = await readFile(artifact);
  } catch {
    fail("Build dist/plugin.mjs before running conformance", "artifact_invalid");
  }
  if (bytes.length === 0 || bytes.length > MAX_ARTIFACT_BYTES) {
    fail(`plugin.mjs must be between 1 and ${MAX_ARTIFACT_BYTES} bytes`, "artifact_invalid");
  }
  const text = bytes.toString("utf8");
  if (!Buffer.from(text, "utf8").equals(bytes)) fail("plugin.mjs must be valid UTF-8", "artifact_invalid");
  return bytes;
}

async function listTools(artifact) {
  const tools = [];
  const names = new Set();
  const cursors = new Set();
  let cursor;
  for (let page = 0; page < 16; page += 1) {
    const response = await runOperation(artifact, {
      method: "tools/list",
      params: cursor === undefined ? {} : { cursor },
    });
    const result = object(response.result, "tools/list result", "tool_catalog_invalid");
    if (!Array.isArray(result.tools)) fail("tools/list result must contain a tools array", "tool_catalog_invalid");
    if (tools.length + result.tools.length > MAX_TOOLS) fail(`tools/list returned more than ${MAX_TOOLS} tools`, "limit_exceeded");
    for (const value of result.tools) {
      const tool = object(value, "tool definition", "tool_catalog_invalid");
      if (typeof tool.name !== "string" || tool.name.length === 0 || names.has(tool.name)) {
        fail("tool names must be non-empty and unique", "tool_catalog_invalid");
      }
      if (Buffer.byteLength(tool.name) > MAX_TOOL_NAME_BYTES) fail(`tool name exceeds ${MAX_TOOL_NAME_BYTES} bytes`, "limit_exceeded");
      for (const field of ["title", "description"]) {
        if (tool[field] != null && typeof tool[field] !== "string") fail(`tool ${field} must be a string`, "tool_catalog_invalid");
      }
      if (tool.annotations != null) object(tool.annotations, "tool annotations", "tool_catalog_invalid");
      names.add(tool.name);
      const inputSchema = object(tool.inputSchema, "tool input schema", "tool_catalog_invalid");
      if (containsKey(inputSchema, "x-mcp-header")) fail("tool uses unsupported x-mcp-header", "tool_catalog_invalid");
      const schemaBytes = Buffer.byteLength(JSON.stringify(inputSchema)) +
        (tool.outputSchema === undefined ? 0 : Buffer.byteLength(JSON.stringify(tool.outputSchema)));
      if (schemaBytes > MAX_TOOL_SCHEMA_BYTES) fail(`tool schemas exceed ${MAX_TOOL_SCHEMA_BYTES} bytes`, "limit_exceeded");
      compileSchema(inputSchema);
      if (tool.outputSchema !== undefined) compileSchema(tool.outputSchema);
      tools.push(tool);
    }
    if (result.nextCursor == null) return tools;
    if (typeof result.nextCursor !== "string" || result.nextCursor.length === 0 || cursors.has(result.nextCursor)) {
      fail("tools/list cursors must be non-empty and unique", "tool_catalog_invalid");
    }
    if (Buffer.byteLength(result.nextCursor) > MAX_CURSOR_BYTES) fail(`tool cursor exceeds ${MAX_CURSOR_BYTES} bytes`, "limit_exceeded");
    cursors.add(result.nextCursor);
    cursor = result.nextCursor;
  }
  fail("tools/list exceeded 16 pages", "limit_exceeded");
}

function validateCallResult(value) {
  const result = object(value, "tools/call result");
  if (!Array.isArray(result.content)) fail("tools/call result must contain a content array");
  if (result.content.length > MAX_CONTENT_ITEMS) fail(`tools/call returned more than ${MAX_CONTENT_ITEMS} content items`, "limit_exceeded");
  if (result.isError !== undefined && typeof result.isError !== "boolean") fail("tools/call isError must be boolean");
}

async function runOperation(artifact, operation) {
  const home = await mkdtemp(resolve(tmpdir(), "paybox-plugin-conformance-"));
  try {
    return await withProcess(process.execPath, [artifact], {
      cwd: dirname(artifact),
      env: { HOME: home, NODE_ENV: "production", NO_COLOR: "1", PATH: process.env.PATH ?? "/usr/local/bin:/usr/bin:/bin" },
    }, async (child, closed, stop) => {
      let stdoutBytes = 0;
      let stderrBytes = 0;
      child.stdout.on("data", (chunk) => {
        stdoutBytes += chunk.byteLength;
        if (stdoutBytes > MAX_STDOUT_BYTES) stop(`plugin stdout exceeded ${MAX_STDOUT_BYTES} bytes`, "limit_exceeded");
      });
      child.stderr.on("data", (chunk) => {
        stderrBytes += chunk.byteLength;
        if (stderrBytes > MAX_STDERR_BYTES) stop(`plugin stderr exceeded ${MAX_STDERR_BYTES} bytes`, "limit_exceeded");
      });
      const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
      try {
        send(child, {
          jsonrpc: "2.0", id: 2, method: operation.method,
          params: { ...operation.params, _meta: {
            "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientInfo": { name: "paybox-plugin-conformance", version: release.version },
            "io.modelcontextprotocol/clientCapabilities": {},
          } },
        });
        return await receive(lines[Symbol.asyncIterator](), 2);
      } catch (error) {
        if (error.code === "startup_failed") await closed;
        throw error;
      } finally { lines.close(); }
    }, OPERATION_TIMEOUT_MS);
  } finally { await rm(home, { recursive: true, force: true }); }
}

function send(child, message) {
  child.stdin.write(`${JSON.stringify(message)}\n`);
}

async function receive(messages, expectedId) {
  for (;;) {
    const next = await messages.next();
    if (next.done) fail("plugin exited before returning an MCP response", "startup_failed");
    let message;
    try {
      message = JSON.parse(next.value);
    } catch {
      fail("plugin stdout contained non-JSON output");
    }
    if (message?.jsonrpc !== "2.0" || typeof message !== "object" || Array.isArray(message)) {
      fail("plugin returned an invalid JSON-RPC message");
    }
    if (!("id" in message)) continue;
    if (message.id !== expectedId) fail("plugin response id did not match request");
    if (("result" in message) === ("error" in message)) fail("plugin response must contain exactly one of result or error");
    if ("error" in message) fail("plugin returned a JSON-RPC error", "tool_failed");
    if (message.result?.resultType !== "complete") fail("MCP 2026-07-28 resultType must be complete");
    return message;
  }
}

function object(value, label, code = "protocol_invalid") {
  if (value === null || typeof value !== "object" || Array.isArray(value)) fail(`${label} must be an object`, code);
  return value;
}

function containsKey(value, key) {
  if (Array.isArray(value)) return value.some((entry) => containsKey(entry, key));
  if (value === null || typeof value !== "object") return false;
  return Object.entries(value).some(([entryKey, entry]) => entryKey === key || containsKey(entry, key));
}

function fail(message, code = "protocol_invalid") {
  throw new ConformanceError(message, code);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === realpathSync(process.argv[1])) {
  await reportConformance(() => testArtifact());
}
