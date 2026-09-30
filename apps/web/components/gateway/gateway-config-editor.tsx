'use client'

import dynamic from 'next/dynamic'

const TextSurface = dynamic(
  () => import('@/components/ui/text-surface').then((module) => module.TextSurface),
  { ssr: false },
)

interface GatewayConfigEditorProps {
  value: string
  hasText: boolean
  onChange: (value: string) => void
  onCopy: () => void
}

/** Lazy CodeMirror boundary for the optional gateway JSON drawer. */
export function GatewayConfigEditor({
  value,
  hasText,
  onChange,
  onCopy,
}: GatewayConfigEditorProps) {
  return (
    <div className="min-h-[420px] flex-1">
      <TextSurface
        path="gateway-config.json"
        value={value}
        mode="edit"
        language="json"
        diagnostics={hasText ? undefined : []}
        onChange={onChange}
        onCopy={onCopy}
      />
    </div>
  )
}
