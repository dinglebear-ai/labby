import type { LauncherEntry } from "@/lib/launcherCatalog";

export interface SchemaFormField {
  name: string;
  type: "string" | "number" | "integer" | "boolean";
  required: boolean;
  description: string;
  enumValues?: string[];
}

export function schemaFormFields(entry: LauncherEntry | null | undefined): SchemaFormField[] {
  const schema = entry?.inputSchema;
  if (!schema || typeof schema !== "object" || Array.isArray(schema)) return [];
  const record = schema as Record<string, unknown>;
  if (record.type !== undefined && record.type !== "object") return [];
  const properties = record.properties;
  if (!properties || typeof properties !== "object" || Array.isArray(properties)) return [];
  const required = new Set(
    Array.isArray(record.required)
      ? record.required.filter((item): item is string => typeof item === "string")
      : [],
  );
  return Object.entries(properties)
    .map(([name, value]) => fieldFromProperty(name, value, required.has(name)))
    .filter((field): field is SchemaFormField => Boolean(field));
}

export function updateSchemaFormJson(
  jsonText: string,
  field: SchemaFormField,
  rawValue: string,
): string {
  const current = parseObject(jsonText);
  if (!current) return jsonText;
  if (rawValue === "" && !field.required) {
    delete current[field.name];
  } else {
    current[field.name] = coerceFieldValue(field, rawValue);
  }
  return JSON.stringify(current, null, 2);
}

export function schemaFieldValue(jsonText: string, field: SchemaFormField): string {
  const current = parseSchemaFormObject(jsonText);
  return schemaFieldValueFromObject(current, field);
}

export function schemaFieldValueFromObject(
  current: Record<string, unknown> | null,
  field: SchemaFormField,
): string {
  if (!current) return "";
  const value = current[field.name];
  if (value === undefined || value === null) return "";
  return typeof value === "string" ? value : String(value);
}

export function parseSchemaFormObject(jsonText: string): Record<string, unknown> | null {
  return parseObject(jsonText);
}

function fieldFromProperty(
  name: string,
  value: unknown,
  required: boolean,
): SchemaFormField | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  const type = normalizeType(record.type);
  if (!type) return null;
  const enumValues = Array.isArray(record.enum)
    ? record.enum.filter((item): item is string => typeof item === "string")
    : undefined;
  return {
    name,
    type,
    required,
    description: typeof record.description === "string" ? record.description : "",
    enumValues: enumValues?.length ? enumValues : undefined,
  };
}

function normalizeType(type: unknown): SchemaFormField["type"] | null {
  if (type === "string" || type === "number" || type === "integer" || type === "boolean")
    return type;
  if (Array.isArray(type)) {
    return (
      type.find(
        (item) =>
          item === "string" || item === "number" || item === "integer" || item === "boolean",
      ) ?? null
    );
  }
  return null;
}

function coerceFieldValue(field: SchemaFormField, rawValue: string): unknown {
  if (field.type === "boolean") return rawValue === "true";
  if (field.type === "integer") {
    const parsed = Number(rawValue);
    return Number.isFinite(parsed) && Number.isInteger(parsed) ? parsed : rawValue;
  }
  if (field.type === "number") {
    const parsed = Number(rawValue);
    return Number.isFinite(parsed) ? parsed : rawValue;
  }
  return rawValue;
}

function parseObject(jsonText: string): Record<string, unknown> | null {
  try {
    const parsed = jsonText.trim() ? JSON.parse(jsonText) : {};
    return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? { ...parsed } : {};
  } catch {
    return null;
  }
}
