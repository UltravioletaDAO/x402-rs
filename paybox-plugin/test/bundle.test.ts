// dist/plugin.mjs, once `pnpm test` (or `pnpm build`) wrote it: within this plugin's own ceiling, with
// no AJV, with nothing that signs, proposes a transaction or pays, and without the payment rails of the
// facilitator. Without a build the checks of the bundle are skipped and say so; the source checks
// always run.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";

import { PLUGIN_VERSION } from "../src/server.ts";
import { ROOT } from "./harness.ts";

/** Far under PayBox's 10 MiB: a cascaron with one host has no reason to weigh more. */
export const MAX_BUNDLE_BYTES = 500_000;
const BUNDLE = `${ROOT}dist/plugin.mjs`;
// What signing, proposing a transaction or paying would bring in: libraries, wallet calls and the
// starter's proposal helpers. Words a read's schema uses (maxAmount, the escrow's paymentInfo field)
// are data, not a signer, and are not on this list.
const SIGNING = ["@noble", "secp256k1", "signTypedData", "eth_sign", "eth_sendTransaction", "privateKey", "transactionProposal", "proposeX402"]; // anti-riel: las palabras que este test afirma que NO estan en el bundle ni en la fuente
const RAILS = ["x402_accepts", "x402_verify", "x402_settle"];

test("the bundle fits, has no AJV, nothing that signs or pays, and no payment rail", { skip: !existsSync(BUNDLE) && "no dist/plugin.mjs: run pnpm build" }, () => {
  const bytes = readFileSync(BUNDLE);
  assert.ok(bytes.length <= MAX_BUNDLE_BYTES, `${bytes.length} bytes`);
  const text = bytes.toString("utf8");
  assert.equal(/ajv/i.test(text), false, "AJV is bundled");
  for (const word of [...SIGNING, ...RAILS]) assert.equal(text.includes(word), false, word);
  assert.ok(text.includes(PLUGIN_VERSION));
});

test("the source has nothing that signs or pays, and names no payment rail", () => {
  for (const file of readdirSync(`${ROOT}src`)) {
    const source = readFileSync(`${ROOT}src/${file}`, "utf8");
    for (const word of [...SIGNING, ...RAILS]) assert.equal(source.includes(word), false, `${file}: ${word}`);
  }
});
