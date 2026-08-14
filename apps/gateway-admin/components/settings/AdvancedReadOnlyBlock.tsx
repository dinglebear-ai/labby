import type { SettingsFieldSpec, SettingsState } from '@/lib/api/setup-client'
import {
  SettingsCard,
  SettingsRow,
  SETTINGS_MULTILINE_CONTROL_STYLE,
} from './SettingsChrome'

export function AdvancedReadOnlyBlock({
  state,
  fields,
}: {
  state: SettingsState
  fields: SettingsFieldSpec[]
}): React.ReactElement {
  return (
    <SettingsCard
      title="Read-only advanced config"
      description="Complex and dangerous settings are visible here redacted. Typed editors are separate follow-up work."
    >
      {fields.map((field) => (
        <SettingsRow
          key={field.key}
          layout="stacked"
          label={field.label}
          description={field.description}
          control={
            <pre
              className="max-h-72 overflow-auto"
              style={{ ...SETTINGS_MULTILINE_CONTROL_STYLE, fontSize: 11, margin: 0 }}
            >
              {JSON.stringify(state.values[field.key] ?? null, null, 2)}
            </pre>
          }
        />
      ))}
    </SettingsCard>
  )
}
