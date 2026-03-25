/**
 * Minimal JSON Schema type stub (subset of JSON Schema Draft 7).
 * Avoids pulling in @types/json-schema for a handful of tool parameter definitions.
 */
export interface JSONSchema7 {
  type?: string | string[];
  properties?: Record<string, JSONSchema7>;
  required?: string[];
  description?: string;
  items?: JSONSchema7;
  enum?: unknown[];
  default?: unknown;
  additionalProperties?: boolean | JSONSchema7;
  [key: string]: unknown;
}
