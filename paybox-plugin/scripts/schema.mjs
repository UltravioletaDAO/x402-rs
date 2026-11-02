import Ajv2020 from "ajv/dist/2020.js";
import { ConformanceError } from "./report.mjs";

export function compileSchema(schema) {
  try {
    const validate = new Ajv2020({ strict: false, validateFormats: false, logger: false }).compile(schema);
    if (validate.$async) throw new Error("Asynchronous schemas are unsupported");
    return validate;
  } catch {
    throw new ConformanceError("Tool schema must be valid, self-contained JSON Schema 2020-12", "schema_invalid");
  }
}

export function validateSample(tool, argumentsValue, result) {
  const input = compileSchema(tool.inputSchema);
  if (!input(argumentsValue)) throw new ConformanceError("Sample arguments do not match the tool input schema", "arguments_invalid");
  if (tool.outputSchema !== undefined && result !== undefined) {
    const output = compileSchema(tool.outputSchema);
    if (!("structuredContent" in result) || !output(result.structuredContent)) {
      throw new ConformanceError("Sample structuredContent does not match the declared output schema", "result_invalid");
    }
  }
}
