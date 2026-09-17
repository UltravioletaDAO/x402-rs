import { readdirSync } from "node:fs";
import { join } from "node:path";

export function vectorPaths(dir = new URL("./vectors/", import.meta.url).pathname) {
  return readdirSync(dir)
    .filter((f) => f.endsWith(".json") && f !== "index.json")
    .sort()
    .map((f) => join(dir, f));
}
