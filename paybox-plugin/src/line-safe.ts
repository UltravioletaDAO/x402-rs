// MCP over stdio is one JSON message per line. JSON.stringify leaves U+2028 and
// U+2029 raw inside strings, and a host that splits lines the way Node's
// readline does (the starter's own conformance check does) breaks the message
// there. The facilitator's reads carry text other people wrote (a seller's
// metadata in DX402 evidence, an error from a chain's node), so one of them could
// otherwise cut the answer in two. Written as their \u escapes they are the same
// JSON: neither character can appear in JSON outside a string.
import { Writable } from "node:stream";

// Built from code points: a literal escape of these two in source is easy to lose
// on the way to disk, and the raw characters are invisible in review.
const LINE_SEPARATORS = new RegExp(`[${String.fromCharCode(0x2028)}${String.fromCharCode(0x2029)}]`, "g");

export function lineSafe(output: Writable): Writable {
  return new Writable({
    decodeStrings: false,
    write(chunk: string | Buffer, _encoding, callback) {
      output.write(escapeLineSeparators(typeof chunk === "string" ? chunk : chunk.toString("utf8")), callback);
    },
  });
}

/** The text as lineSafe writes it. */
export function escapeLineSeparators(text: string): string {
  return text.replace(LINE_SEPARATORS, (c) => `\\u${c.charCodeAt(0).toString(16)}`);
}
