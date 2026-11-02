// The tooling files are the PayBox starter 1.0.4, byte for byte: an upgrade replaces them all
// together, never one at a time. Three files differ from the starter's manifest, the ones its guide
// says to edit: package.json (this plugin's name, version, build alias and test scripts),
// paybox-plugin.json (the sample call) and src/plugin.ts (this plugin's entry). And one is left out
// on purpose: src/paybox.ts, the starter's transaction and x402 proposal helpers, because this plugin
// only reads: it proposes nothing, signs nothing and pays nothing. The lockfile is the starter's, byte
// for byte: no dependency was added.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";

import { ROOT } from "./harness.ts";

const manifest = JSON.parse(readFileSync(`${ROOT}starter-manifest.json`, "utf8")) as {
  version: string;
  files: Record<string, string>;
};
const EDITED = new Set(["package.json", "paybox-plugin.json", "src/plugin.ts"]);
const LEFT_OUT = new Set(["src/paybox.ts"]);

// sha256 of https://developer.paybox.sh/plugin-starter/v1.0.4/manifest.json, one value a reviewer
// can check with curl (the same the other plugins of the stack pin).
const OFFICIAL_MANIFEST_SHA256 = "9c663d5753a64e1b664a0dcc9ec6425cd08d737c861314b078ee23fad5373ea5";

const sha256 = (file: string) => createHash("sha256").update(readFileSync(`${ROOT}${file}`)).digest("hex");

test("starter-manifest.json is the official 1.0.4 manifest", () => {
  assert.equal(sha256("starter-manifest.json"), OFFICIAL_MANIFEST_SHA256);
  assert.equal(manifest.version, "1.0.4");
});

test("the starter's tooling files are unmodified, the lockfile included", () => {
  const checked = Object.keys(manifest.files).filter((file) => !EDITED.has(file) && !LEFT_OUT.has(file));
  assert.equal(checked.length, 10);
  assert.ok(checked.includes("pnpm-lock.yaml"));
  for (const file of checked) assert.equal(sha256(file), manifest.files[file], file);
});

test("the proposal helpers are not here: nothing proposes, signs or pays", () => {
  for (const file of LEFT_OUT) assert.equal(existsSync(`${ROOT}${file}`), false, file);
});

test("package.json keeps the starter's dependencies, exactly", () => {
  const pkg = JSON.parse(readFileSync(`${ROOT}package.json`, "utf8"));
  assert.deepEqual(pkg.dependencies, { "@modelcontextprotocol/server": "2.0.0", zod: "4.4.3" });
  assert.deepEqual(pkg.devDependencies, { "@types/node": "24.12.2", esbuild: "0.28.2", tsx: "4.23.12", typescript: "5.9.3", ajv: "8.20.0" });
  assert.equal(pkg.version, "0.1.0");
});
