// After `pnpm test` built dist/plugin.mjs: its size against this plugin's own ceiling (far under
// PayBox's 10 MiB), its SHA-256, and no AJV in it (src/sdk-shims.ts keeps it out).
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

const MAX_BUNDLE_BYTES = 500_000;
const bytes = readFileSync("dist/plugin.mjs");
const report = {
  bytes: bytes.length,
  sha256: createHash("sha256").update(bytes).digest("hex"),
  max: MAX_BUNDLE_BYTES,
  ajv: /ajv/i.test(bytes.toString("utf8")),
};
console.log(JSON.stringify(report));
if (report.bytes > MAX_BUNDLE_BYTES || report.ajv) process.exit(1);
