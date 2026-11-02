// The catalog this plugin lists: the facilitator's, generated (src/catalogo.generated.ts) by the
// facilitator itself from the tools/list POST /mcp answers, keeping the tools of class `lectura`.
// Nothing here is written by hand: a tool, its schema or its text changes in src/mcp.rs and comes back
// with the regeneration in README.md. This module only checks the shape once, at startup, so that a
// broken file fails loudly instead of listing something else, and so that a tool which is not a read
// never reaches tools/list, whatever the file says.
import { CATALOGO } from "./catalogo.generated.js";

/** The `_meta` key under which the facilitator declares each tool's house class. */
export const CLASS_META_KEY = "uvd/clase";
/** The one class this plugin relays. */
export const READ_CLASS = "lectura";

/** A tool as the facilitator's tools/list publishes it, as is. */
export interface ToolDefinition {
  name: string;
  title?: string;
  description?: string;
  inputSchema: Record<string, unknown>;
  outputSchema?: Record<string, unknown>;
  annotations?: Record<string, unknown>;
  _meta?: Record<string, unknown>;
}

/** A tools/call result, as the facilitator answers it. */
export interface CallResult {
  content: unknown[];
  structuredContent?: Record<string, unknown>;
  isError?: boolean;
  _meta?: Record<string, unknown>;
}

export interface Catalog {
  /** The path of the facilitator's MCP on the one host this plugin calls. */
  ruta: string;
  tools: ToolDefinition[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function fail(what: string): never {
  throw new Error(`src/catalogo.generated.ts: ${what}`);
}

/** The generated catalog, checked: every field this plugin reads, and only reads. */
export function readCatalog(value: unknown): Catalog {
  if (!isRecord(value) || value.esquema !== 1) fail("not esquema 1");
  const { ruta, tools } = value;
  if (typeof ruta !== "string" || !ruta.startsWith("/") || ruta.startsWith("//")) fail("ruta is not a path");
  if (!Array.isArray(tools) || tools.length === 0) fail("no tools");
  const names = new Set<string>();
  for (const tool of tools) {
    if (!isRecord(tool) || typeof tool.name !== "string" || !isRecord(tool.inputSchema)) fail("a tool without name or inputSchema");
    if (names.has(tool.name)) fail(`${tool.name} twice`);
    names.add(tool.name);
    if (!isRecord(tool._meta) || tool._meta[CLASS_META_KEY] !== READ_CLASS) fail(`${tool.name} is not of class ${READ_CLASS}`);
    if (!isRecord(tool.annotations) || tool.annotations.readOnlyHint !== true) fail(`${tool.name} is not read-only`);
  }
  return value as unknown as Catalog;
}

export const CATALOG: Catalog = readCatalog(CATALOGO);
