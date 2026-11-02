// tools/list is the catalog the facilitator generated, as is: the tools of class `lectura` of its own
// tools/list, every one read-only, and never a payment rail. The catalog is checked against the
// facilitator's whole tools/list (test/fixtures/facilitator-tools.json, generated with it), so this
// holds even if someone edits the catalog by hand.
import { test } from "node:test";
import assert from "node:assert/strict";

import { CATALOG, CLASS_META_KEY, READ_CLASS, readCatalog } from "../src/catalogo.ts";
import { CATALOGO } from "../src/catalogo.generated.ts";
import { TOOL_NAMES } from "../src/server.ts";
import { deps, facilitatorTools, fakeFetch, rpc } from "./harness.ts";

/** The reads this plugin relays, as the encargo names them: x402_supported and the four of X-1. */
const READS = ["x402_supported", "x402_networks", "x402_chain_health", "x402_escrow_onchain_state", "x402_durable_evidence"];
const RAILS = ["x402_accepts", "x402_verify", "x402_settle"];

test("tools/list is the generated catalog, unchanged and in one page", async () => {
  const response = await rpc(deps(fakeFetch().fetchImpl), "tools/list");
  assert.equal(response.error, undefined, JSON.stringify(response.error));
  assert.deepEqual(response.result.tools, CATALOG.tools);
  assert.equal(response.result.nextCursor, undefined);
  assert.deepEqual(TOOL_NAMES, CATALOG.tools.map((t) => t.name));
});

test("the catalog is exactly the reads of the facilitator's tools/list, definitions and all", () => {
  const listed = facilitatorTools();
  assert.deepEqual(
    listed.map((t) => t.name),
    ["x402_supported", ...RAILS, "x402_networks", "x402_chain_health", "x402_escrow_onchain_state", "x402_durable_evidence"],
  );
  const reads = listed.filter((t) => t._meta?.[CLASS_META_KEY] === READ_CLASS);
  assert.deepEqual(CATALOG.tools, reads);
  assert.deepEqual(TOOL_NAMES, READS);
});

test("every tool only reads, and none is a payment rail", () => {
  for (const tool of CATALOG.tools) {
    assert.equal(tool.annotations?.readOnlyHint, true, tool.name);
    assert.equal(tool._meta?.[CLASS_META_KEY], READ_CLASS, tool.name);
    assert.match(tool.name, /^[A-Za-z0-9_-]{1,64}$/);
  }
  for (const rail of RAILS) assert.ok(!TOOL_NAMES.includes(rail), rail);
  assert.doesNotMatch(JSON.stringify(CATALOG.tools), /x402_accepts|x402_verify|x402_settle/);
});

test("the escaped salt is the same string once read", () => {
  const escrow = CATALOG.tools.find((t) => t.name === "x402_escrow_onchain_state");
  const salt = (escrow?.inputSchema as any).examples[0].paymentInfo.salt;
  assert.match(salt, /^0x[0-9a-f]{64}$/);
});

test("a broken catalog fails at startup instead of listing something else", () => {
  const good = CATALOGO as Record<string, unknown>;
  const tools = good.tools as Record<string, any>[];
  assert.throws(() => readCatalog({ ...good, esquema: 2 }), /esquema 1/);
  assert.throws(() => readCatalog({ ...good, ruta: "https://elsewhere.example/mcp" }), /ruta/);
  assert.throws(() => readCatalog({ ...good, ruta: "//elsewhere.example/mcp" }), /ruta/);
  assert.throws(() => readCatalog({ ...good, tools: [] }), /no tools/);
  assert.throws(() => readCatalog({ ...good, tools: [...tools, tools[1]] }), /twice/);
  // A payment rail slipped into the catalog by hand never reaches tools/list.
  const settle = facilitatorTools().find((t) => t.name === "x402_settle");
  assert.throws(() => readCatalog({ ...good, tools: [...tools, settle] }), /not of class lectura/);
  const relabeled = { ...settle, _meta: { [CLASS_META_KEY]: READ_CLASS } };
  assert.throws(() => readCatalog({ ...good, tools: [...tools, relabeled] }), /not read-only/);
});
