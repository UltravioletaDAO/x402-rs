// What the MCP SDK's Node shim (@modelcontextprotocol/server/_shims) provides, without AJV. esbuild
// substitutes this module for the SDK's (package.json, "build": --alias), which keeps AJV out of
// dist/plugin.mjs, as the other plugins of the stack do.
//
// The SDK uses DefaultJsonSchemaValidator to check a client's elicitation answer and in
// fromJsonSchema. This plugin does neither: it answers tools/list with the definitions the facilitator
// generated and forwards tools/call, and the facilitator validates the arguments. If something ever
// calls it, it fails loudly instead of validating nothing. test/bundle.test.ts keeps AJV out of the build.
import process from "node:process";

export class DefaultJsonSchemaValidator {
  getValidator(): never {
    throw new Error("JSON Schema validation is not bundled in this plugin: the facilitator validates the arguments");
  }
}

export { process };
