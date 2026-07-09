import Ajv, { type AnySchema, type ValidateFunction } from "ajv";

import type { LauncherEntry } from "@/lib/launcherCatalog";

const ajv = new Ajv({ allErrors: true, strict: false });
const validators = new Map<string, ValidateFunction>();

export interface LauncherValidationResult {
  valid: boolean;
  message?: string;
}

export function validateLauncherParams(entry: LauncherEntry, params: unknown): LauncherValidationResult {
  if (!params || typeof params !== "object" || Array.isArray(params)) {
    return { valid: false, message: "Params must be a JSON object" };
  }
  if (!entry.inputSchema) return { valid: true };
  const validator = validatorFor(entry);
  if (!validator) return { valid: true };
  if (validator(params)) return { valid: true };
  return {
    valid: false,
    message: validator.errors?.[0]?.message ? `Params ${validator.errors[0].message}` : "Params do not match schema",
  };
}

export function exampleLauncherParams(_entry: LauncherEntry): string {
  return "{}";
}

export function redactLauncherParams(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(redactLauncherParams);
  if (!value || typeof value !== "object") return value;
  const result: Record<string, unknown> = {};
  for (const [key, child] of Object.entries(value)) {
    result[key] = sensitiveKey(key) ? "[REDACTED]" : redactLauncherParams(child);
  }
  return result;
}

function validatorFor(entry: LauncherEntry): ValidateFunction | null {
  const key = `${entry.id}:${entry.schemaFingerprint ?? "none"}`;
  const cached = validators.get(key);
  if (cached) return cached;
  try {
    if (!schemaLooksSupported(entry.inputSchema)) return null;
    const schema = entry.inputSchema as AnySchema;
    if (!ajv.validateSchema(schema)) return null;
    const validator = ajv.compile(schema);
    validators.set(key, validator);
    return validator;
  } catch {
    return null;
  }
}

function schemaLooksSupported(schema: unknown): boolean {
  if (!schema || typeof schema !== "object" || Array.isArray(schema)) return false;
  const record = schema as Record<string, unknown>;
  const type = record.type;
  if (type === undefined) return true;
  const allowed = new Set(["object", "array", "string", "number", "integer", "boolean", "null"]);
  if (typeof type === "string") return allowed.has(type);
  if (Array.isArray(type)) return type.every((item) => typeof item === "string" && allowed.has(item));
  return false;
}

function sensitiveKey(key: string): boolean {
  const lower = key.toLowerCase();
  return (
    lower.includes("token") ||
    lower.includes("secret") ||
    lower.includes("password") ||
    lower.includes("apikey") ||
    lower.includes("authorization") ||
    lower.includes("key")
  );
}
