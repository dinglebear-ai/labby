'use client'

/**
 * Inline "Add Server" flow, ported from the mock's in-palette add sheet:
 * a command/endpoint line with transport detection, a name field, contextual
 * auth pills (None / Bearer / OAuth) plus a token-env input for Bearer, an
 * env field for stdio, and Resources / Prompts proxy switches.
 *
 * It is backed by the real `createGateway` mutation — nothing here is a mock.
 * "Full Dialog" hands off to the real gateway editor on `/gateways`; the
 * compact "Add & Probe" path remains backed by the same create mutation.
 */

import { useState } from 'react'
import { Globe, Loader2, Terminal } from 'lucide-react'
import { toast } from 'sonner'

import {
  buildAddServerInput,
  detectPaletteAddTransport,
  type PaletteAddAuth,
} from '@/lib/app-command-palette'
import type { CreateGatewayInput } from '@/lib/types/gateway'

const AUTH_PILLS: Array<{ key: PaletteAddAuth; label: string }> = [
  { key: 'none', label: 'None' },
  { key: 'bearer', label: 'Bearer' },
  { key: 'oauth', label: 'OAuth' },
]

export function PaletteAddServer({
  isSubmitting,
  onOpenFullDialog,
  onSubmit,
}: {
  isSubmitting: boolean
  onOpenFullDialog: () => void
  onSubmit: (input: CreateGatewayInput) => void
}) {
  const [name, setName] = useState('')
  const [target, setTarget] = useState('')
  const [auth, setAuth] = useState<PaletteAddAuth>('none')
  const [tokenEnv, setTokenEnv] = useState('')
  const [env, setEnv] = useState('')
  const [proxyResources, setProxyResources] = useState(true)
  const [proxyPrompts, setProxyPrompts] = useState(true)

  const transport = detectPaletteAddTransport(target)

  function submit() {
    const input = buildAddServerInput({
      name,
      target,
      auth,
      tokenEnv,
      env,
      proxyResources,
      proxyPrompts,
    })
    if (!input) {
      toast.error('Enter an endpoint URL or a runnable command first')
      return
    }
    onSubmit(input)
  }

  return (
    <div className="pal-add">
      <div className="pal-add-cmd">
        <span className="pal-add-caret">❯</span>
        <input
          autoFocus
          className="pal-add-cmdinput"
          value={target}
          onChange={(event) => setTarget(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') {
              event.preventDefault()
              submit()
            }
          }}
          placeholder="https://host/mcp  ·  uvx my-mcp-server"
          aria-label="Command or endpoint"
        />
        {transport ? (
          <span className="pal-add-kind">
            {transport === 'http' ? <Globe size={9} /> : <Terminal size={9} />}
            {transport === 'http' ? 'HTTP' : 'STDIO'}
          </span>
        ) : null}
      </div>

      <div className="pal-add-grid">
        <div className="pal-add-col">
          <label className="pal-add-label" htmlFor="pal-add-name">
            Name
          </label>
          <input
            id="pal-add-name"
            className="pal-add-input"
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="labby-mcp"
            aria-label="Server name"
          />
        </div>

        {transport === 'http' ? (
          <div className="pal-add-col">
            <span className="pal-add-label">Authentication</span>
            <div className="flex min-w-0 items-center gap-[5px]">
              {AUTH_PILLS.map((pill) => (
                <button
                  key={pill.key}
                  type="button"
                  className="pal-add-pill"
                  aria-pressed={auth === pill.key}
                  onClick={() => setAuth(pill.key)}
                >
                  {pill.label}
                </button>
              ))}
              {auth === 'bearer' ? (
                <input
                  className="pal-add-tokeninput"
                  value={tokenEnv}
                  onChange={(event) => setTokenEnv(event.target.value)}
                  placeholder="TOKEN_ENV_VAR"
                  aria-label="Bearer token env var"
                />
              ) : null}
            </div>
          </div>
        ) : null}

        {transport === 'stdio' ? (
          <div className="pal-add-col">
            <label className="pal-add-label" htmlFor="pal-add-env">
              Environment
            </label>
            <input
              id="pal-add-env"
              className="pal-add-input"
              style={{ fontSize: '10.5px' }}
              value={env}
              onChange={(event) => setEnv(event.target.value)}
              placeholder="KEY=value, KEY=value"
              aria-label="Environment variables"
            />
          </div>
        ) : null}

        {transport === null ? (
          <div className="pal-add-col">
            <span
              className="pal-add-label"
              style={{ color: 'color-mix(in srgb, var(--aurora-text-muted) 55%, transparent)' }}
            >
              Options
            </span>
            <span className="pal-add-note">
              Auth or env options appear once the transport is detected.
            </span>
          </div>
        ) : null}
      </div>

      <div className="pal-add-foot">
        <PaletteSwitch
          label="Resources"
          description="Proxy discovered MCP resources downstream."
          checked={proxyResources}
          onToggle={() => setProxyResources((value) => !value)}
        />
        <PaletteSwitch
          label="Prompts"
          description="Proxy discovered MCP prompts downstream."
          checked={proxyPrompts}
          onToggle={() => setProxyPrompts((value) => !value)}
        />
        <span className="pal-grow" />
        <button type="button" className="pal-btn" onClick={onOpenFullDialog}>
          Full Dialog
        </button>
        <button
          type="button"
          className="pal-btn"
          data-primary="1"
          disabled={isSubmitting || transport === null}
          onClick={submit}
        >
          {isSubmitting ? <Loader2 size={11} className="mr-1 inline animate-spin" /> : null}
          Add &amp; Probe
        </button>
      </div>
    </div>
  )
}

function PaletteSwitch({
  label,
  description,
  checked,
  onToggle,
}: {
  label: string
  description: string
  checked: boolean
  onToggle: () => void
}) {
  return (
    <button
      type="button"
      role="switch"
      className="pal-switch"
      aria-checked={checked}
      aria-label={label}
      title={description}
      onClick={onToggle}
    >
      <span className="pal-switch-track">
        <span className="pal-switch-thumb" />
      </span>
      <span className="pal-switch-label">{label}</span>
    </button>
  )
}
