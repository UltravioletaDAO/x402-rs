// The entry PayBox runs: the starter's (1.0.4) with this plugin's server and lineSafe stdout.
import { serveStdio, StdioServerTransport } from "@modelcontextprotocol/server/stdio";

import { lineSafe } from "./line-safe.js";
import { createServer } from "./server.js";

serveStdio(() => createServer({ fetch: globalThis.fetch }), {
  legacy: "reject",
  transport: new StdioServerTransport(process.stdin, lineSafe(process.stdout)),
});
