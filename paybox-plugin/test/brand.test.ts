// The plugin carries the DAO's name and the facilitator's one host, and nobody else's: it is the
// facilitator's own plugin, not a copy of another one of the stack. server/discover says who it is;
// the source names one host.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";

import { INSTRUCTIONS } from "../src/instructions.ts";
import { PLUGIN_NAME, PLUGIN_TITLE, USER_AGENT } from "../src/server.ts";
import { deps, fakeFetch, ROOT, rpc } from "./harness.ts";

const OTHERS = /emp[o0]rium|execution\.market|meshrelay|describe\.net|karmakadabra/i;

test("server/discover names the DAO's facilitator", async () => {
  const response = await rpc(deps(fakeFetch().fetchImpl), "server/discover");
  assert.equal(response.error, undefined, JSON.stringify(response.error));
  // MCP 2026-07-28 carries serverInfo in the result's _meta, not as a field of its own.
  const info = response.result._meta["io.modelcontextprotocol/serverInfo"];
  assert.equal(info.name, PLUGIN_NAME);
  assert.equal(info.title, PLUGIN_TITLE);
  assert.match(PLUGIN_TITLE, /^Ultravioleta DAO /);
  assert.match(INSTRUCTIONS, /^Ultravioleta DAO /);
  assert.match(USER_AGENT, /^uvd-facilitator-paybox-plugin\//);
  assert.doesNotMatch(JSON.stringify(response.result), OTHERS);
});

test("the source, the package and the catalog name one host, the facilitator's, and no other plugin", () => {
  const hosts = new Set<string>();
  for (const file of readdirSync(`${ROOT}src`)) {
    const source = readFileSync(`${ROOT}src/${file}`, "utf8");
    assert.doesNotMatch(source, OTHERS, file);
    if (file === "catalogo.generated.ts") continue;
    for (const url of source.match(/https?:\/\/[A-Za-z0-9.-]+/g) ?? []) hosts.add(url);
  }
  assert.deepEqual([...hosts], ["https://facilitator.ultravioletadao.xyz"]);
  for (const file of ["package.json", "paybox-plugin.json"]) {
    assert.doesNotMatch(readFileSync(`${ROOT}${file}`, "utf8"), OTHERS, file);
  }
});
