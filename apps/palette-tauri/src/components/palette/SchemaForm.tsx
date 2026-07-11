import type { LauncherEntry } from "@/lib/launcherCatalog";
import { schemaFieldValue, schemaFormFields, updateSchemaFormJson } from "@/lib/schemaForm";

interface SchemaFormProps {
  action: LauncherEntry | null;
  value: string;
  onChange: (value: string) => void;
}

export function SchemaForm({ action, value, onChange }: SchemaFormProps) {
  const fields = schemaFormFields(action);
  if (fields.length === 0) return null;

  return (
    <section className="schema-form" aria-label="Parameters">
      {fields.map((field) => {
        const id = `schema-field-${field.name}`;
        const fieldValue = schemaFieldValue(value, field);
        return (
          <label className="schema-field" key={field.name} htmlFor={id}>
            <span className="schema-field-label">
              {field.name}
              {field.required ? <span aria-hidden="true">*</span> : null}
            </span>
            {field.type === "boolean" ? (
              <select
                id={id}
                value={fieldValue || "false"}
                onChange={(event) => onChange(updateSchemaFormJson(value, field, event.target.value))}
              >
                <option value="false">false</option>
                <option value="true">true</option>
              </select>
            ) : field.enumValues ? (
              <select
                id={id}
                value={fieldValue}
                onChange={(event) => onChange(updateSchemaFormJson(value, field, event.target.value))}
              >
                {!field.required ? <option value="" /> : null}
                {field.enumValues.map((option) => (
                  <option value={option} key={option}>
                    {option}
                  </option>
                ))}
              </select>
            ) : (
              <input
                id={id}
                value={fieldValue}
                type={field.type === "string" ? "text" : "number"}
                step={field.type === "integer" ? "1" : "any"}
                onChange={(event) => onChange(updateSchemaFormJson(value, field, event.target.value))}
              />
            )}
          </label>
        );
      })}
    </section>
  );
}
