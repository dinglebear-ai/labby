'use client'

import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from 'react'
import {
  AlertCircle,
  ChevronRight,
  ClipboardPaste,
  FileJson2,
  KeyRound,
  Loader2,
  Play,
  Route,
  Settings2,
  ShieldCheck,
  ShieldOff,
  X,
} from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Tabs, TabsContent } from '@/components/ui/tabs'
import { FieldGroup, Field, FieldLabel, FieldDescription } from '@/components/ui/field'
import {
  useGatewayMutations,
  useProtectedMcpRoutes,
  useServiceConfig,
  useSupportedServices,
} from '@/lib/hooks/use-gateways'
import type {
  Gateway,
  CreateGatewayInput,
  UpdateGatewayInput,
  TransportType,
  ProtectedMcpRouteInput,
} from '@/lib/types/gateway'
import { toast } from 'sonner'
import { cn, getErrorMessage } from '@/lib/utils'
import { defaultGatewayBearerEnvName, validateBearerTokenEnvName } from '@/lib/gateway-env'
import { validateGatewayName } from '@/lib/utils/gateway-name'
import { isAbortError } from '@/lib/api/service-action-client'
import { GatewayApiError } from '@/lib/api/gateway-client-core'
import {
  initialGatewayAuthMode,
  normalizeProtectedPublicPath,
  protectedRouteForGateway,
  protectedRoutePathInputValue,
  type GatewayAuthMode,
} from '@/lib/gateway-protected-route'
import { upstreamOauthApi } from '@/lib/api/upstream-oauth-client'
import { useUpstreamOauthStatus } from '@/lib/hooks/use-upstream-oauth'
import type { OAuthConnectState } from '@/lib/types/upstream-oauth'
import { formatStdioCommandLine, parseStdioCommandLine } from '@/lib/stdio-command'
import { Badge } from '@/components/ui/badge'
import {
  SERVICE_ENV_PREFIXES,
} from '@/lib/branding/service-brands'
import {
  gatewayFormUiReducer,
  initialGatewayFormUiState,
  type GatewayFormUiState,
} from './gateway-form-state'
import { oauthConnectButtonLabel, shouldAutoConnectOauth } from './gateway-form-oauth'
import { GatewayConfigEditor } from './gateway-config-editor'
import {
  GatewaySaveCompensationError,
  runGatewaySaveTransaction,
  type GatewaySaveRollback,
} from './gateway-save-transaction'
import { serviceFields } from './gateway-service-fields'
import { GatewayCustomConnectionForm } from './gateway-custom-connection-form'
import { GatewayLabServiceForm } from './gateway-lab-service-form'

export type { GatewaySaveRollback } from './gateway-save-transaction'

export { oauthConnectButtonLabel, shouldAutoConnectOauth } from './gateway-form-oauth'

interface GatewayFormDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  gateway: Gateway | null
  onSave: (input: CreateGatewayInput | UpdateGatewayInput) => Promise<GatewaySaveRollback | void>
}

type FormMode = 'custom' | 'lab'
type GatewayAuthSource = 'paste' | 'env'

const PROTECTED_MCP_PUBLIC_HOST = process.env.NEXT_PUBLIC_PROTECTED_MCP_HOST?.trim() ?? ''
const PROTECTED_MCP_PUBLIC_HOST_LABEL = PROTECTED_MCP_PUBLIC_HOST || 'protected host not configured'
const PROTECTED_ROUTE_SCOPES = ['mcp:read', 'mcp:write']
const gatewayInputClassName =
  'border-aurora-border-strong bg-aurora-page-bg/80 shadow-[var(--aurora-highlight-medium)] placeholder:text-aurora-text-muted/70 hover:border-aurora-accent-primary/35 focus-visible:bg-aurora-control-surface'

function valuePreview(fieldName: string, preview?: string | null) {
  return preview ?? (fieldName.endsWith('_URL') ? 'http://localhost' : '')
}

export function parseEnvText(text: string): { pairs: Record<string, string>; detectedServices: string[] } {
  const pairs: Record<string, string> = {}
  for (const line of text.split('\n')) {
    const trimmed = line.trim()
    if (!trimmed || trimmed.startsWith('#')) continue
    const eqIdx = trimmed.indexOf('=')
    if (eqIdx < 1) continue
    const key = trimmed.slice(0, eqIdx).trim()
    const val = trimmed.slice(eqIdx + 1).trim()
    pairs[key] = val
  }
  const found = new Set<string>()
  for (const key of Object.keys(pairs)) {
    for (const [prefix, serviceKey] of Object.entries(SERVICE_ENV_PREFIXES)) {
      if (key.startsWith(`${prefix}_`)) {
        found.add(serviceKey)
      }
    }
    if (found.size === 0 || ![...found].some((service) => key.toLowerCase().startsWith(`${service}_`))) {
      const match = key.match(/^([A-Za-z][A-Za-z0-9]*)_(URL|URI|TOKEN|API_KEY|KEY|SECRET|HOST|BASE_URL)$/)
      if (match) {
        found.add(match[1]!.toLowerCase())
      }
    }
  }
  return { pairs, detectedServices: [...found] }
}

function envPrefixForGatewayName(name: string): string | null {
  const trimmed = name.trim()
  if (!trimmed) return null
  const knownPrefix = Object.entries(SERVICE_ENV_PREFIXES).find(([, key]) => key === trimmed)?.[0]
  if (knownPrefix) return knownPrefix
  const prefix = trimmed.toUpperCase().replace(/[^A-Z0-9]+/g, '_').replace(/^_+|_+$/g, '')
  return prefix || null
}

function formatEnvPairs(pairs: Record<string, string>): string {
  return Object.entries(pairs)
    .filter(([key, value]) => key.trim() && value.trim())
    .map(([key, value]) => `${key.trim()}=${value.trim()}`)
    .join('\n')
}

export function buildEnvTextFromGatewayForm({
  name,
  transport,
  url,
  stdioEnv,
}: {
  name: string
  transport: TransportType
  url: string
  stdioEnv: Record<string, string>
}): string {
  if (transport === 'stdio') {
    return formatEnvPairs(stdioEnv)
  }
  if (Object.keys(stdioEnv).length > 0) {
    return formatEnvPairs(stdioEnv)
  }
  const prefix = envPrefixForGatewayName(name)
  const trimmedUrl = url.trim()
  if (!prefix || !trimmedUrl) return ''
  return `${prefix}_URL=${trimmedUrl}`
}

export function parseGatewayJsonEntry(text: string): { name: string; config: Record<string, unknown> } | null {
  const parsed = JSON.parse(text) as Record<string, unknown>
  const root = parsed.mcpServers
  const entries =
    root && typeof root === 'object' && !Array.isArray(root)
      ? Object.entries(root as Record<string, unknown>)
      : Object.entries(parsed)
  if (entries.length !== 1) return null
  const [name, config] = entries[0]!
  if (!config || typeof config !== 'object' || Array.isArray(config)) return null
  return { name, config: config as Record<string, unknown> }
}

function parseGatewayEnvObject(value: unknown): Record<string, string> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {}
  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>)
      .filter((entry): entry is [string, string] => typeof entry[1] === 'string')
      .map(([key, envValue]) => [key.trim(), envValue.trim()] as const)
      .filter(([key, envValue]) => key.length > 0 && envValue.length > 0),
  )
}

const emptyCustomState = {
  transport: 'http' as TransportType,
  name: '',
  url: '',
  command: '',
  bearerTokenEnv: '',
  proxyResources: true,
  proxyPrompts: true,
  proxyMcpUi: true,
}

export function GatewayFormDialog({
  open,
  onOpenChange,
  gateway,
  onSave,
}: GatewayFormDialogProps) {
  const isEditing = !!gateway
  const isLabGateway = gateway?.source === 'in_process'
  const prevOpenRef = useRef(false)
  const abortControllerRef = useRef<AbortController | null>(null)
  const probeInfoRef = useRef<{ registration_strategy: string; scopes?: string[] } | null>(null)
  const currentProbeUrlRef = useRef('')
  const nameAutoRef = useRef(false)
  const skipUrlOauthResetRef = useRef(false)
  const autoOauthAttemptedForRef = useRef<string | null>(null)
  const protectedRouteHydratedForRef = useRef<string | null>(null)
  const protectedRouteTouchedRef = useRef(false)
  const { data: supportedServices } = useSupportedServices()
  const { data: protectedRoutes = [] } = useProtectedMcpRoutes()
  const { testGateway, saveServiceConfig, enableVirtualServer, disableVirtualServer, addProtectedRoute, updateProtectedRoute, removeProtectedRoute } =
    useGatewayMutations()

  const [mode, setMode] = useState<FormMode>('custom')
  const [transport, setTransport] = useState<TransportType>('http')
  const [name, setName] = useState('')
  const [url, setUrl] = useState('')
  const [protectedPublicPath, setProtectedPublicPath] = useState('')
  const [command, setCommand] = useState('')
  const [stdioEnv, setStdioEnv] = useState<Record<string, string>>({})
  const [authMode, setAuthMode] = useState<GatewayAuthMode>('none')
  const [authSource, setAuthSource] = useState<GatewayAuthSource>('paste')
  const [bearerTokenEnv, setBearerTokenEnv] = useState('')
  const [bearerTokenValue, setBearerTokenValue] = useState('')
  const [proxyResources, setProxyResources] = useState(true)
  const [proxyPrompts, setProxyPrompts] = useState(true)
  const [proxyMcpUi, setProxyMcpUi] = useState(true)
  const [uiState, dispatchUi] = useReducer(gatewayFormUiReducer, initialGatewayFormUiState)
  const setUi = useCallback(<Key extends keyof GatewayFormUiState,>(
    key: Key,
    value: GatewayFormUiState[Key],
  ) => {
    dispatchUi({ type: 'set', key, value })
  }, [])
  const {
    jsonDrawerOpen,
    isSaving,
    isTesting,
    saveError,
    errors,
    oauthState,
    oauthProbed,
    isProbing,
  } = uiState
  const setJsonDrawerOpen = useCallback(
    (value: boolean) => setUi('jsonDrawerOpen', value),
    [setUi],
  )
  const [jsonText, setJsonText] = useState('')
  const [jsonValid, setJsonValid] = useState(false)
  const syncingRef = useRef(false)
  const [envText, setEnvText] = useState('')

  const [selectedService, setSelectedService] = useState('')
  const [serviceValues, setServiceValues] = useState<Record<string, string>>({})
  const [enableServer, setEnableServer] = useState(true)

  const setIsSaving = useCallback((value: boolean) => setUi('isSaving', value), [setUi])
  const setIsTesting = useCallback((value: boolean) => setUi('isTesting', value), [setUi])
  const setSaveError = useCallback((value: string | null) => setUi('saveError', value), [setUi])
  const setErrors = useCallback(
    (value: Record<string, string>) => setUi('errors', value),
    [setUi],
  )
  const setOauthState = useCallback(
    (value: OAuthConnectState) => setUi('oauthState', value),
    [setUi],
  )
  const setOauthProbed = useCallback(
    (value: GatewayFormUiState['oauthProbed']) => setUi('oauthProbed', value),
    [setUi],
  )
  const setIsProbing = useCallback((value: boolean) => setUi('isProbing', value), [setUi])

  useEffect(() => () => abortControllerRef.current?.abort(), [])

  const requestOpenChange = (nextOpen: boolean) => {
    if (!nextOpen) {
      abortControllerRef.current?.abort()
      abortControllerRef.current = null
    }
    onOpenChange(nextOpen)
  }

  const serviceMeta = useMemo(
    () => supportedServices?.find((service) => service.key === selectedService) ?? null,
    [selectedService, supportedServices],
  )
  const serviceEnvFields = useMemo(() => serviceFields(serviceMeta), [serviceMeta])
  const existingProtectedRoute = useMemo(
    () => protectedRouteForGateway(gateway, protectedRoutes, PROTECTED_MCP_PUBLIC_HOST),
    [gateway, protectedRoutes],
  )
  const protectedRoutePathOptions = useMemo(() => {
    const seen = new Set<string>()
    return protectedRoutes
      .filter((route) => route.enabled && route.public_host === PROTECTED_MCP_PUBLIC_HOST)
      .map((route) => route.public_path)
      .filter((path) => {
        if (seen.has(path)) return false
        seen.add(path)
        return true
      })
      .sort((left, right) => left.localeCompare(right))
  }, [protectedRoutes])
  const { data: serviceConfig } = useServiceConfig(mode === 'lab' && selectedService ? selectedService : null)

  const oauthUpstream = oauthState.kind === 'authorizing'
    || oauthState.kind === 'connected'
    || oauthState.kind === 'discovered'
    || oauthState.kind === 'blocked'
    ? (oauthState as { upstream: string }).upstream
    : null
  const { data: oauthStatus } = useUpstreamOauthStatus(
    oauthState.kind === 'authorizing' ? oauthUpstream : null,
    { pollWhilePending: oauthState.kind === 'authorizing' },
  )

  useEffect(() => {
    if (oauthState.kind === 'authorizing' && oauthStatus?.authenticated) {
      const info = probeInfoRef.current
      setOauthState({
        kind: 'connected',
        upstream: oauthState.upstream,
        registration_strategy: info?.registration_strategy ?? 'dynamic',
        scopes: info?.scopes,
      })
    }
  }, [oauthState, oauthStatus?.authenticated, setOauthState])

  // Auto-probe the URL for OAuth support when transport is HTTP and URL looks valid.
  // Resets probed state and authMode when URL changes so stale OAuth option disappears.
  useEffect(() => {
    currentProbeUrlRef.current = url.trim()
    if (transport !== 'http' || !url.trim()) {
      setOauthProbed(null)
      setIsProbing(false)
      if (authMode === 'oauth' && !protectedPublicPath.trim()) setAuthMode('none')
      return
    }
    const requestedUrl = url.trim()
    setOauthProbed(null)
    const ac = new AbortController()
    const timer = setTimeout(() => {
      setIsProbing(true)
      upstreamOauthApi.probe(requestedUrl, ac.signal).then((result) => {
        if (ac.signal.aborted || currentProbeUrlRef.current !== requestedUrl) return
        setOauthProbed(result); setIsProbing(false)
      }).catch((err: unknown) => {
        if (isAbortError(err)) return
        if (ac.signal.aborted || currentProbeUrlRef.current !== requestedUrl) return
        setOauthProbed({ oauth_discovered: false, upstream: '' }); setIsProbing(false)
      })
    }, 600)
    return () => {
      ac.abort()
      setIsProbing(false)
      clearTimeout(timer)
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [url, transport])

  useEffect(() => {
    if (!oauthProbed) return
    if (!shouldAutoConnectOauth({
      open,
      isEditing,
      transport,
      authMode,
      oauthDiscovered: oauthProbed.oauth_discovered,
      upstream: oauthProbed.upstream,
    })) return

    const attemptKey = `${url.trim()}::${oauthProbed.upstream}`
    if (autoOauthAttemptedForRef.current === attemptKey) return
    autoOauthAttemptedForRef.current = attemptKey

    setAuthMode('oauth')
    void runOauthConnect({ authTab: null, auto: true, probeOverride: oauthProbed })
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [authMode, isEditing, oauthProbed, open, transport, url])

  // Auto-fill the name from the URL hostname when the user hasn't typed a name yet.
  useEffect(() => {
    if (isEditing || transport !== 'http' || !url.trim()) return
    try {
      const hostname = new URL(url).hostname.replace(/^www\./, '')
      const slug = hostname.replace(/[^a-z0-9]+/gi, '-').toLowerCase().replace(/^-+|-+$/g, '')
      if (!slug) return
      setName((prev) => {
        if (!prev || nameAutoRef.current) {
          nameAutoRef.current = true
          return slug
        }
        return prev
      })
    } catch {
      // invalid URL, skip
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [url])

  async function runOauthConnect({
    authTab,
    auto,
    probeOverride,
  }: {
    authTab: Window | null
    auto: boolean
    probeOverride?: NonNullable<typeof oauthProbed>
  }) {
    if (!url.trim()) return
    setOauthState({ kind: 'probing' })
    try {
      const requestedUpstream = name.trim() || undefined
      const reusableProbe = probeOverride?.oauth_discovered
        && (!requestedUpstream || probeOverride.upstream === requestedUpstream)
      const probe = reusableProbe
        ? probeOverride
        : await upstreamOauthApi.probe(url.trim(), undefined, requestedUpstream)
      if (!probe.oauth_discovered) {
        authTab?.close()
        setOauthState({ kind: 'error', message: 'This server does not advertise OAuth support' })
        return
      }
      setOauthProbed(probe)
      setOauthState({ kind: 'discovered', upstream: probe.upstream, issuer: probe.issuer, scopes: probe.scopes })
      probeInfoRef.current = { registration_strategy: probe.registration_strategy ?? 'dynamic', scopes: probe.scopes }
      const { authorization_url } = await upstreamOauthApi.start(probe.upstream)

      const targetTab = authTab ?? window.open(authorization_url, '_blank')
      if (!targetTab || targetTab.closed) {
        setOauthState(
          auto
            ? { kind: 'blocked', upstream: probe.upstream, issuer: probe.issuer, scopes: probe.scopes }
            : { kind: 'error', message: 'Authorization tab was closed. Please try again.' },
        )
        return
      }
      if (authTab) {
        authTab.location.href = authorization_url
      }
      setOauthState({ kind: 'authorizing', upstream: probe.upstream })
    } catch (err: unknown) {
      authTab?.close()
      setOauthState({ kind: 'error', message: err instanceof Error ? err.message : 'OAuth connection failed' })
    }
  }

  async function handleOauthConnect() {
    if (!url.trim()) return
    // Open a blank tab synchronously in the click handler so browsers allow it.
    const authTab = window.open('about:blank', '_blank')
    await runOauthConnect({ authTab, auto: false, probeOverride: oauthProbed ?? undefined })
  }

  useEffect(() => {
    const wasOpen = prevOpenRef.current
    prevOpenRef.current = open
    if (!open || wasOpen) return

    setJsonDrawerOpen(false)

    if (gateway) {
      if (gateway.source === 'in_process') {
        setMode('lab')
        setSelectedService(gateway.id)
        setEnableServer(gateway.enabled ?? true)
      } else {
        setMode('custom')
        setTransport(gateway.transport === 'in_process' ? 'http' : gateway.transport)
        setName(gateway.name)
        const protectedRoute = protectedRouteForGateway(gateway, protectedRoutes, PROTECTED_MCP_PUBLIC_HOST)
        const protectedPath = protectedRoutePathInputValue(protectedRoute)
        const initialAuthMode = initialGatewayAuthMode(gateway, protectedRoute)
        protectedRouteHydratedForRef.current = protectedRoute ? gateway.id : null
        protectedRouteTouchedRef.current = false
        skipUrlOauthResetRef.current = initialAuthMode === 'oauth'
        setUrl(gateway.config.url || '')
        setProtectedPublicPath(protectedPath)
        setCommand(formatStdioCommandLine(gateway.config.command, gateway.config.args))
        setStdioEnv(gateway.config.env ?? {})
        setAuthMode(initialAuthMode)
        if (gateway.config.oauth_enabled) {
          setOauthState({ kind: 'connected', upstream: gateway.name, registration_strategy: 'preregistered', scopes: undefined })
          setOauthProbed({ oauth_discovered: true, upstream: gateway.name })
        } else {
          setOauthState({ kind: 'idle' })
          setOauthProbed(null)
        }
        setAuthSource(gateway.config.bearer_token_env ? 'env' : 'paste')
        setBearerTokenEnv(gateway.config.bearer_token_env || '')
        setBearerTokenValue('')
        setProxyResources(gateway.config.proxy_resources ?? true)
        setProxyPrompts(gateway.config.proxy_prompts ?? true)
        setProxyMcpUi(gateway.config.proxy_mcp_ui ?? true)
      }
      } else {
        setMode('custom')
        setTransport(emptyCustomState.transport)
        setName(emptyCustomState.name)
        setUrl(emptyCustomState.url)
        setProtectedPublicPath('')
        protectedRouteHydratedForRef.current = null
        protectedRouteTouchedRef.current = false
        autoOauthAttemptedForRef.current = null
        setCommand(emptyCustomState.command)
        setStdioEnv({})
        setAuthMode('none')
        setAuthSource('paste')
        setBearerTokenEnv(emptyCustomState.bearerTokenEnv)
        setBearerTokenValue('')
        setProxyResources(emptyCustomState.proxyResources)
        setProxyPrompts(emptyCustomState.proxyPrompts)
        setProxyMcpUi(emptyCustomState.proxyMcpUi)
        setSelectedService('')
        setServiceValues({})
        setEnableServer(true)
        nameAutoRef.current = false
      }
    setErrors({})
  }, [open, gateway, protectedRoutes, setErrors, setJsonDrawerOpen, setOauthProbed, setOauthState])

  useEffect(() => {
    if (!open || !gateway || gateway.source === 'in_process') return
    if (protectedRouteHydratedForRef.current === gateway.id) return
    if (protectedRouteTouchedRef.current) return
    const protectedRoute = existingProtectedRoute
    if (!protectedRoute) return

    protectedRouteHydratedForRef.current = gateway.id
    setProtectedPublicPath(protectedRoutePathInputValue(protectedRoute))
    setAuthMode((current) => (current === 'bearer' ? current : 'oauth'))
  }, [existingProtectedRoute, gateway, open])

  useEffect(() => {
    setServiceValues({})
  }, [selectedService])

  useEffect(() => {
    if (skipUrlOauthResetRef.current) {
      skipUrlOauthResetRef.current = false
      return
    }
    if (
      isEditing &&
      gateway?.config.oauth_enabled &&
      url.trim() === (gateway.config.url ?? '').trim()
    ) {
      return
    }
    autoOauthAttemptedForRef.current = null
    setOauthState({ kind: 'idle' })
    setOauthProbed(null)
  }, [gateway, isEditing, setOauthProbed, setOauthState, url])

  // Stdio connections don't support upstream authentication, so clear all OAuth
  // and bearer state whenever the transport is stdio. If a protected public path
  // is also absent we additionally reset the auth mode to 'none'.
  useEffect(() => {
    if (transport !== 'stdio') return
    if (!protectedPublicPath.trim()) {
      setAuthMode('none')
    }
    setOauthState({ kind: 'idle' })
    setOauthProbed(null)
    setBearerTokenEnv('')
    setBearerTokenValue('')
  }, [protectedPublicPath, setOauthProbed, setOauthState, transport])

  useEffect(() => {
    if (!serviceMeta || !serviceConfig) return

    const nextValues: Record<string, string> = {}
    for (const field of serviceEnvFields) {
      const configField = serviceConfig.fields.find((item) => item.name === field.name)
      nextValues[field.name] = valuePreview(field.name, configField?.value_preview)
    }
    setServiceValues(nextValues)
  }, [serviceConfig, serviceEnvFields, serviceMeta])

  const validateCustom = () => {
    const newErrors: Record<string, string> = {}

    const nameError = validateGatewayName(name.trim())
    if (nameError) {
      newErrors.name = nameError
    }

    if (transport === 'http') {
      if (!url.trim()) {
        newErrors.url = 'URL is required'
      } else {
        try {
          new URL(url)
        } catch {
          newErrors.url = 'Invalid URL format'
        }
      }

    } else {
      try {
        parseStdioCommandLine(command)
      } catch (error) {
        newErrors.command = error instanceof Error ? error.message : 'Invalid command'
      }
    }

    if (protectedPublicPath.trim()) {
      try {
        normalizeProtectedPublicPath(protectedPublicPath)
      } catch (error) {
        newErrors.protectedPublicPath = error instanceof Error
          ? error.message
          : 'Invalid protected route path'
      }
      if (!PROTECTED_MCP_PUBLIC_HOST) {
        newErrors.protectedPublicPath = 'Set NEXT_PUBLIC_PROTECTED_MCP_HOST before creating protected routes'
      }
    }

    if (transport === 'http' && authMode === 'oauth' && !protectedPublicPath.trim()) {
      if (oauthState.kind !== 'connected' && !oauthStatus?.authenticated) {
        newErrors.oauth = 'Complete OAuth authorization before saving'
      }
    }

    if (transport === 'http' && authMode === 'bearer') {
      if (authSource === 'env') {
        if (!bearerTokenEnv.trim()) {
          newErrors.bearerTokenEnv = 'Environment variable name is required'
        } else {
          const bearerTokenEnvError = validateBearerTokenEnvName(bearerTokenEnv)
          if (bearerTokenEnvError) {
            newErrors.bearerTokenEnv = bearerTokenEnvError
          }
        }
      } else {
        if (!bearerTokenValue.trim()) {
          newErrors.bearerTokenValue = 'Bearer token is required'
        }

        if (bearerTokenEnv.trim()) {
          const bearerTokenEnvError = validateBearerTokenEnvName(bearerTokenEnv)
          if (bearerTokenEnvError) {
            newErrors.bearerTokenEnv = bearerTokenEnvError
          }
        }
      }
    }

    setErrors(newErrors)
    return Object.keys(newErrors).length === 0
  }

  const validateLab = () => {
    const newErrors: Record<string, string> = {}
    if (!selectedService) {
      newErrors.service = 'Choose a Lab service'
    }
    for (const field of serviceMeta?.required_env ?? []) {
      const configField = serviceConfig?.fields.find((item) => item.name === field.name)
      const keepExistingSecret = field.secret && configField?.present && !serviceValues[field.name]?.trim()
      if (!keepExistingSecret && !serviceValues[field.name]?.trim()) {
        newErrors[field.name] = `${field.name} is required`
      }
    }
    setErrors(newErrors)
    return Object.keys(newErrors).length === 0
  }

  const buildInput = (): CreateGatewayInput => {
    const stdio = transport === 'stdio' ? parseStdioCommandLine(command) : null
    const authEnabled = transport === 'http'
    const preserveExistingOauth = isEditing && gateway?.config.oauth_enabled && authMode === 'oauth'
    const oauthConfig =
      authEnabled
      && authMode === 'oauth'
      && oauthState.kind === 'connected'
      && oauthState.registration_strategy !== 'unknown'
      && !preserveExistingOauth
        ? { registration_strategy: oauthState.registration_strategy, scopes: oauthState.scopes }
        : undefined
    return {
      name,
      transport,
      config: {
        ...(transport === 'http'
          ? {
              url,
              ...(Object.keys(stdioEnv).length > 0 ? { env: stdioEnv } : {}),
            }
          : {
              command: stdio?.command,
              args: stdio && stdio.args.length > 0 ? stdio.args : undefined,
              env: Object.keys(stdioEnv).length > 0 ? stdioEnv : undefined,
            }),
        bearer_token_env: !authEnabled
          ? undefined
          : authMode === 'none' || authMode === 'oauth'
            ? null
            : authSource === 'env'
              ? bearerTokenEnv
              : bearerTokenEnv || undefined,
        bearer_token_value:
          authEnabled && authMode === 'bearer' && authSource === 'paste'
            ? bearerTokenValue
            : undefined,
        oauth: oauthConfig,
        proxy_resources: proxyResources,
        proxy_prompts: proxyPrompts,
        proxy_mcp_ui: proxyMcpUi,
      },
    }
  }

  const buildProtectedRouteInput = (publicPath: string): ProtectedMcpRouteInput => ({
    name: name.trim(),
    enabled: true,
    public_host: PROTECTED_MCP_PUBLIC_HOST,
    public_path: publicPath,
    upstream: name.trim(),
    backend_url: '',
    scopes: PROTECTED_ROUTE_SCOPES,
    health_path: null,
  })

  const saveProtectedRoute = async (publicPath: string, signal?: AbortSignal): Promise<void> => {
    const route = buildProtectedRouteInput(publicPath)
    const existingPathRoute = protectedRoutes.find(
      (item) =>
        item.enabled &&
        item.public_host === route.public_host &&
        item.public_path === route.public_path,
    )
    if (
      existingPathRoute &&
      existingPathRoute.name !== route.name &&
      existingPathRoute.name !== existingProtectedRoute?.name
    ) {
      throw new GatewayApiError(
        `Protected route ${route.public_path} is already assigned to ${existingPathRoute.upstream ?? existingPathRoute.name}. Choose a different path or edit that route first.`,
        409,
      )
    }

    if (existingProtectedRoute) {
      await updateProtectedRoute(existingProtectedRoute.name, {
        ...route,
        name: existingProtectedRoute.name,
      }, signal)
      return
    }

    try {
      await addProtectedRoute(route, signal)
    } catch (error) {
      if (error instanceof GatewayApiError && error.status === 409) {
        await updateProtectedRoute(route.name, route, signal)
        return
      }
      throw error
    }
  }

  const removeExistingProtectedRouteIfCleared = async (
    publicPath: string,
    signal?: AbortSignal,
  ): Promise<void> => {
    if (!existingProtectedRoute) return
    // Only remove when the protected-route field was explicitly cleared.
    // If publicPath is non-empty, saveProtectedRoute already handled the
    // update/replace; deleting here would silently discard the just-saved route.
    if (publicPath) return
    await removeProtectedRoute(existingProtectedRoute.name, signal)
  }

  const handleTest = async () => {
    if (isSaving) return
    if (!gateway || gateway.source === 'in_process') {
      toast.info('Save and enable the server first, then test from the detail page.')
      return
    }

    if (!validateCustom()) return

    const controller = new AbortController()
    abortControllerRef.current = controller

    setIsTesting(true)
    try {
      const result = await testGateway(gateway.id, controller.signal)
      if (controller.signal.aborted) return
      if (result.severity === 'warning') {
        toast.warning(result.detail || result.message)
      } else if (result.success) {
        toast.success(`Connection successful: ${result.latency_ms}ms latency`)
      } else {
        toast.error(`Connection failed: ${result.error || result.message}`)
      }
    } catch (error) {
      if (isAbortError(error)) return
      toast.error(getErrorMessage(error, 'Failed to test connection'))
    } finally {
      setIsTesting(false)
    }
  }

  const handleSaveLab = async (): Promise<boolean> => {
    if (!validateLab() || !selectedService) return false

    const values = Object.fromEntries(
      Object.entries(serviceValues).filter(([field, value]) => {
        const configField = serviceConfig?.fields.find((item) => item.name === field)
        if (configField?.secret && configField.present && !value.trim()) {
          return false
        }
        return true
      }),
    )

    await saveServiceConfig(selectedService, values)
    if (enableServer) {
      await enableVirtualServer(selectedService)
    } else {
      await disableVirtualServer(selectedService)
    }
    return true
  }

  const handleSave = async () => {
    if (isTesting) return

    const controller = new AbortController()
    abortControllerRef.current = controller

    setIsSaving(true)
    try {
      if (mode === 'lab') {
        const saved = await handleSaveLab()
        if (controller.signal.aborted) return
        if (!saved) {
          return
        }
        toast.success(isEditing ? 'Lab server updated successfully' : 'Lab server configured successfully')
        requestOpenChange(false)
        return
      }

      if (!validateCustom()) return
      setSaveError(null)
      const normalizedProtectedPath = normalizeProtectedPublicPath(protectedPublicPath)
      const reusedProtectedRoute = Boolean(
        normalizedProtectedPath &&
        protectedRoutes.some(
          (route) =>
            route.enabled &&
            route.public_host === PROTECTED_MCP_PUBLIC_HOST &&
            route.public_path === normalizedProtectedPath &&
            route.name !== name.trim() &&
            route.name !== existingProtectedRoute?.name,
        ),
      )
      await runGatewaySaveTransaction(
        () => onSave(buildInput()),
        async () => {
          if (normalizedProtectedPath) {
            await saveProtectedRoute(normalizedProtectedPath, controller.signal)
          } else {
            await removeExistingProtectedRouteIfCleared(normalizedProtectedPath, controller.signal)
          }
        },
      )
      if (controller.signal.aborted) return
      toast.success(
        normalizedProtectedPath
          ? reusedProtectedRoute
            ? `Server saved and joined https://${PROTECTED_MCP_PUBLIC_HOST}${normalizedProtectedPath}`
            : `Server saved and protected at https://${PROTECTED_MCP_PUBLIC_HOST}${normalizedProtectedPath}`
          : isEditing
            ? 'Server updated successfully'
            : 'Server created successfully',
      )
      requestOpenChange(false)
    } catch (error) {
      if (error instanceof GatewaySaveCompensationError) {
        const message = getErrorMessage(error.rollbackError, error.message)
        setSaveError(message)
        toast.error(message)
        return
      }
      if (isAbortError(error)) return
      if (error instanceof GatewayApiError && error.status === 409) {
        setSaveError(error.message)
        return
      }
      toast.error(
        getErrorMessage(
          error,
          mode === 'lab'
            ? 'Failed to save Lab server'
            : isEditing
              ? 'Failed to update server'
              : 'Failed to create server',
        ),
      )
    } finally {
      setIsSaving(false)
    }
  }

  const toggleJsonDrawer = () => {
    const next = !jsonDrawerOpen
    setJsonDrawerOpen(next)
  }

  const applyEnvTextToForm = (text: string) => {
    const { pairs, detectedServices } = parseEnvText(text)
    setStdioEnv(pairs)
    if (transport === 'stdio') {
      if (!name.trim() && detectedServices[0]) setName(detectedServices[0])
      return Object.keys(pairs).length > 0
    }
    const detected = detectedServices[0]
    if (!detected) return Object.keys(pairs).length > 0
    const prefix = Object.entries(SERVICE_ENV_PREFIXES).find(([, key]) => key === detected)?.[0]
    setMode('custom')
    if (prefix) {
      setTransport('http')
      setName(detected)
      const urlKey = `${prefix}_URL`
      if (pairs[urlKey]) setUrl(pairs[urlKey])
    } else {
      setName((current) => current.trim() || detected)
    }
    return true
  }

  const handleEnvTextChange = (next: string) => {
    setEnvText(next)
    syncingRef.current = true
    applyEnvTextToForm(next)
    setTimeout(() => { syncingRef.current = false }, 0)
  }

  const buildJsonFromForm = (): object | null => {
    const n = name.trim()
    if (!n) return null
    const cfg: Record<string, unknown> = {}
    if (transport === 'http') {
      const u = url.trim()
      if (u) cfg.url = u
      if (Object.keys(stdioEnv).length > 0) cfg.env = stdioEnv
    } else {
      const trimmed = command.trim()
      if (trimmed) {
        try {
          const parsed = parseStdioCommandLine(trimmed)
          cfg.command = parsed.command
          if (parsed.args.length > 0) cfg.args = parsed.args
        } catch {
          cfg.command = trimmed
        }
      }
      if (Object.keys(stdioEnv).length > 0) cfg.env = stdioEnv
    }
    cfg.proxy_resources = proxyResources
    cfg.proxy_prompts = proxyPrompts
    cfg.proxy_mcp_ui = proxyMcpUi
    return { [n]: cfg }
  }

  const isJsonEditorFocused = () => (
    jsonDrawerOpen &&
    typeof document !== 'undefined' &&
    Boolean(document.activeElement?.closest?.('[data-gateway-json-drawer] .cm-editor'))
  )

  const onFormChange = () => {
    if (syncingRef.current || !jsonDrawerOpen || isJsonEditorFocused()) return
    syncingRef.current = true
    const json = buildJsonFromForm()
    if (json) {
      setJsonText(JSON.stringify(json, null, 2))
      setJsonValid(true)
    } else {
      setJsonText('')
      setJsonValid(false)
    }
    // Defer reset so it runs AFTER React flushes batched state — otherwise the
    // useEffect watching [name, url, ...] fires with guard already false and loops.
    setTimeout(() => { syncingRef.current = false }, 0)
  }

  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => { onFormChange() }, [name, url, command, stdioEnv, transport, proxyResources, proxyPrompts, proxyMcpUi, jsonDrawerOpen])

  useEffect(() => {
    if (syncingRef.current || !open || mode !== 'custom') return
    setEnvText(buildEnvTextFromGatewayForm({ name, transport, url, stdioEnv }))
  }, [mode, name, open, stdioEnv, transport, url])

  const parseJsonToForm = (text: string) => {
    if (syncingRef.current) return
    try {
      const entry = parseGatewayJsonEntry(text)
      if (!entry) {
        setJsonValid(false)
        return
      }
      const gatewayName = entry.name
      const cfg = entry.config
      setJsonValid(true)
      syncingRef.current = true
      setName(gatewayName)
      if (typeof cfg.url === 'string') {
        setTransport('http')
        setUrl(cfg.url)
        setStdioEnv(parseGatewayEnvObject(cfg.env))
      } else if (typeof cfg.command === 'string') {
        setTransport('stdio')
        setCommand(formatStdioCommandLine(cfg.command, Array.isArray(cfg.args) ? cfg.args as string[] : []))
        setStdioEnv(parseGatewayEnvObject(cfg.env))
      }
      if (typeof cfg.proxy_resources === 'boolean') setProxyResources(cfg.proxy_resources)
      if (typeof cfg.proxy_prompts === 'boolean') setProxyPrompts(cfg.proxy_prompts)
      if (typeof cfg.proxy_mcp_ui === 'boolean') setProxyMcpUi(cfg.proxy_mcp_ui)
      // Defer reset — same reason as onFormChange: the useEffect fires after React
      // flushes the setName/setUrl/setTransport calls; guard must still be true then.
      setTimeout(() => { syncingRef.current = false }, 0)
    } catch {
      setJsonValid(false)
    }
  }

  const protectedRoutePreview = (() => {
    const normalized = protectedPublicPath.trim()
    if (!normalized) return null
    const publicHost = PROTECTED_MCP_PUBLIC_HOST || '<protected-host>'
    try {
      return `https://${publicHost}${normalizeProtectedPublicPath(normalized)}`
    } catch {
      return `https://${publicHost}/${normalized.replace(/^\/+/, '')}`
    }
  })()
  const jsonHasText = jsonText.trim().length > 0
  const jsonStatusLabel = !jsonHasText ? 'Empty' : jsonValid ? 'Synced' : 'Invalid'
  const jsonStatusClassName = !jsonHasText
    ? 'border-aurora-border-strong bg-aurora-control-surface text-aurora-text-muted'
    : jsonValid
      ? 'border-aurora-success/40 bg-aurora-success-surface text-aurora-success'
      : 'border-aurora-error/40 bg-aurora-error-surface text-aurora-error'
  const jsonTransportLabel = transport === 'http' ? 'HTTP URL' : 'stdio command'

  return (
    <Dialog open={open} onOpenChange={requestOpenChange}>
        <DialogContent
          className={cn(
            'overflow-visible transition-[border-radius] duration-[250ms]',
            'sm:max-w-[540px]',
            jsonDrawerOpen && 'rounded-r-none',
          )}
        >
        <DialogHeader className="shrink-0">
          <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
            <div className="flex min-w-0 flex-col gap-1">
              <DialogTitle>{isEditing ? 'Edit server' : 'Add server'}</DialogTitle>
              <DialogDescription>
                {isEditing
                  ? 'Edit server settings.'
                  : mode === 'lab'
                    ? 'Connect a built-in Lab service.'
                    : 'Connect an upstream MCP server.'}
              </DialogDescription>
            </div>
            <div
              className={cn(
                'flex shrink-0 items-center gap-1.5 sm:mr-8',
                mode === 'custom' ? 'visible' : 'invisible pointer-events-none',
              )}
            >
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={toggleJsonDrawer}
                className={cn(
                  'h-8 rounded-full px-3 text-xs font-medium',
                  jsonDrawerOpen
                    ? 'border-aurora-accent-primary/36 bg-aurora-accent-primary/12 text-aurora-text-primary'
                    : 'border-aurora-border-strong bg-aurora-control-surface text-aurora-text-primary hover:bg-aurora-hover-bg',
                )}
              >
                JSON
              </Button>
            </div>
          </div>
        </DialogHeader>

        <div className="flex-1 min-h-0 overflow-y-auto aurora-scrollbar -mx-6 px-6 pb-24 sm:pb-6">
        <Tabs
          value={mode}
          className="space-y-4"
        >
          <TabsContent value="lab" className="space-y-6">
            <GatewayLabServiceForm
              supportedServices={supportedServices ?? []}
              selectedService={selectedService}
              onSelectService={setSelectedService}
              serviceFields={serviceEnvFields}
              serviceConfig={serviceConfig}
              serviceValues={serviceValues}
              onServiceValuesChange={setServiceValues}
              errors={errors}
              enableServer={enableServer}
              onEnableServerChange={setEnableServer}
            />
          </TabsContent>

          <TabsContent value="custom" className="flex flex-col gap-4">
            <GatewayCustomConnectionForm
              transport={transport}
              onTransportChange={setTransport}
              name={name}
              onNameChange={(next) => { nameAutoRef.current = false; setName(next) }}
              url={url}
              onUrlChange={setUrl}
              command={command}
              onCommandChange={setCommand}
              envText={envText}
              onEnvTextChange={handleEnvTextChange}
              envCount={Object.keys(stdioEnv).length}
              errors={errors}
              isProbing={isProbing}
              oauthDiscovered={Boolean(oauthProbed?.oauth_discovered)}
            />

            <details
              className="group order-3 rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium/50 p-4 shadow-[var(--aurora-highlight-medium)]"
              open={Boolean(protectedPublicPath.trim() || errors.protectedPublicPath)}
            >
              <summary className="flex cursor-pointer select-none list-none items-center justify-between gap-3 text-sm font-semibold text-aurora-text-primary [&::-webkit-details-marker]:hidden">
                <span className="flex min-w-0 items-center gap-2">
                  <ChevronRight className="size-4 shrink-0 transition-transform group-open:rotate-90" />
                  <Route className="size-4 shrink-0 text-aurora-accent-primary" />
                  <span>Protected route</span>
                </span>
                <span className={cn(
                  'max-w-[55%] truncate text-[12px] font-medium',
                  protectedRoutePreview ? 'text-aurora-success' : 'text-aurora-text-muted',
                )}>
                  {protectedRoutePreview ?? 'Not published'}
                </span>
              </summary>
              <div className="mt-4">
                <Field>
                  <FieldLabel htmlFor="protected-public-path">Public path</FieldLabel>
                  <div className="flex overflow-hidden rounded-aurora-1 border border-aurora-border-strong bg-aurora-page-bg/80 shadow-[var(--aurora-highlight-medium)] transition-[border-color,box-shadow,background-color] hover:border-aurora-accent-primary/35 focus-within:border-aurora-accent-primary focus-within:bg-aurora-control-surface focus-within:ring-2 focus-within:ring-aurora-accent-primary/34">
                    <span className="hidden items-center border-r border-aurora-border-strong px-3 text-[13px] text-aurora-text-muted sm:flex">
                      https://{PROTECTED_MCP_PUBLIC_HOST_LABEL}/
                    </span>
                    <Input
                      id="protected-public-path"
                      list="protected-route-path-options"
                      value={protectedPublicPath}
                      onChange={(event) => {
                        protectedRouteTouchedRef.current = true
                        if (!event.target.value.trim() && existingProtectedRoute && authMode === 'oauth') {
                          setAuthMode('none')
                          setOauthState({ kind: 'idle' })
                        }
                        setProtectedPublicPath(event.target.value)
                      }}
                      placeholder="tools"
                      className={cn(
                        'border-0 bg-transparent font-mono text-[13px] focus-visible:ring-0 focus-visible:ring-offset-0',
                        errors.protectedPublicPath && 'text-destructive',
                      )}
                    />
                    <datalist id="protected-route-path-options">
                      {protectedRoutePathOptions.map((path) => (
                        <option key={path} value={path.replace(/^\//, '')}>
                          {`https://${PROTECTED_MCP_PUBLIC_HOST_LABEL}${path}`}
                        </option>
                      ))}
                    </datalist>
                  </div>
                  {errors.protectedPublicPath ? (
                    <p className="text-sm text-destructive">{errors.protectedPublicPath}</p>
                  ) : (
                    <FieldDescription>
                      Optional. Lab OAuth protects this public MCP route.
                    </FieldDescription>
                  )}
                </Field>
              </div>
            </details>

            {transport === 'http' && (
            <details
              className="group order-2 rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium/50 p-4 shadow-[var(--aurora-highlight-medium)]"
              open={Boolean(authMode === 'bearer' || oauthState.kind === 'blocked' || oauthState.kind === 'error' || errors.oauth)}
            >
              <summary className="flex cursor-pointer select-none list-none items-center justify-between gap-3 text-sm font-semibold text-aurora-text-primary [&::-webkit-details-marker]:hidden">
                <span className="flex min-w-0 items-center gap-2">
                  <ChevronRight className="size-4 shrink-0 transition-transform group-open:rotate-90" />
                  {authMode === 'oauth' ? (
                    <ShieldCheck className="size-4 shrink-0 text-aurora-accent-primary" />
                  ) : authMode === 'bearer' ? (
                    <KeyRound className="size-4 shrink-0 text-aurora-warn" />
                  ) : (
                    <ShieldOff className="size-4 shrink-0 text-aurora-text-muted" />
                  )}
                  <span>Upstream auth</span>
                </span>
                <span className={cn(
                  'text-[12px] font-medium',
                  authMode === 'oauth' && oauthState.kind === 'connected'
                    ? 'text-aurora-success'
                    : authMode === 'bearer'
                      ? 'text-aurora-warn'
                      : 'text-aurora-text-muted',
                )}>
                  {authMode === 'none'
                    ? oauthProbed?.oauth_discovered
                      ? 'OAuth detected'
                      : 'No auth'
                    : authMode === 'bearer'
                      ? 'Bearer token'
                      : oauthState.kind === 'connected'
                        ? 'Connected'
                        : 'OAuth'}
                </span>
              </summary>
              <div className="mt-4 space-y-4">
                <Field>
                  <FieldLabel>Authentication</FieldLabel>

                <Select value={authMode} onValueChange={(value) => setAuthMode(value as GatewayAuthMode)}>
                  <SelectTrigger className={cn('w-full', gatewayInputClassName)}>
                    <SelectValue>
                      <span className="flex items-center gap-2">
                        {authMode === 'none' && <ShieldOff className="size-4 text-aurora-text-muted" />}
                        {authMode === 'bearer' && <KeyRound className="size-4 text-aurora-text-muted" />}
                        {authMode === 'oauth' && <ShieldCheck className="size-4 text-aurora-text-muted" />}
                        {authMode === 'none' ? 'No auth' : authMode === 'bearer' ? 'Bearer token' : 'OAuth (MCP)'}
                        {authMode === 'oauth' && oauthProbed?.oauth_discovered && (
                          <Badge
                            variant="secondary"
                            className="ml-1 border-aurora-border-strong bg-aurora-control-surface text-xs text-aurora-text-primary"
                          >
                            Detected
                          </Badge>
                        )}
                      </span>
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent style={{ zIndex: 200 }}>
                    <SelectItem value="none">
                      <span className="flex items-center gap-2">
                        <ShieldOff className="size-4 text-aurora-text-muted" />
                        No auth
                      </span>
                    </SelectItem>
                    <SelectItem value="bearer">
                      <span className="flex items-center gap-2">
                        <KeyRound className="size-4 text-aurora-text-muted" />
                        Bearer token
                      </span>
                    </SelectItem>
                    <SelectItem value="oauth">
                      <span className="flex items-center gap-2">
                        <ShieldCheck className="size-4 text-aurora-text-muted" />
                        OAuth (MCP)
                      </span>
                    </SelectItem>
                  </SelectContent>
                </Select>
                </Field>

                {authMode === 'oauth' && (
                  <div className="flex flex-col gap-3 rounded-aurora-2 border border-aurora-border-strong bg-aurora-control-surface/70 p-4 shadow-[var(--aurora-highlight-medium)]">
                    {oauthState.kind === 'connected' ? (
                      <div className="flex items-center justify-between gap-2">
                        <div className="flex items-center gap-2 text-sm text-aurora-success font-medium">
                          <ShieldCheck className="size-4" />
                          Connected
                          <Badge variant="outline" className="border-aurora-success/40 text-aurora-success ml-1">Authorized</Badge>
                        </div>
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          onClick={() => {
                            setOauthState({ kind: 'idle' })
                            probeInfoRef.current = null
                          }}
                        >
                          Re-authorize
                        </Button>
                      </div>
                    ) : (
                      <>
                        <p className="text-sm text-aurora-text-muted">
                          {!url.trim()
                            ? 'Enter a URL above, then connect.'
                            : oauthState.kind === 'authorizing'
                              ? 'Complete authorization in the new tab.'
                              : oauthState.kind === 'blocked'
                                ? 'OAuth detected. Click to authorize; the browser blocked the automatic popup.'
                                : 'Connect this server via OAuth. A popup will open for you to authorize.'}
                        </p>
                        {oauthState.kind === 'error' && (
                          <div className="flex items-start gap-2 text-sm text-destructive">
                            <AlertCircle className="size-4 mt-0.5 shrink-0" />
                            {oauthState.message}
                          </div>
                        )}
                        <Button
                          type="button"
                          size="sm"
                          variant={oauthState.kind === 'blocked' ? 'default' : 'secondary'}
                          onClick={() => void handleOauthConnect()}
                          disabled={!url.trim() || oauthState.kind === 'probing' || oauthState.kind === 'authorizing'}
                          className={cn(
                            oauthState.kind === 'blocked'
                              && 'ring-2 ring-aurora-accent-primary/45 ring-offset-2 ring-offset-aurora-page-bg',
                          )}
                        >
                          {(oauthState.kind === 'probing' || oauthState.kind === 'authorizing') && (
                            <Loader2 className="size-4 mr-2 animate-spin" />
                          )}
                          {oauthConnectButtonLabel(oauthState)}
                        </Button>
                      </>
                    )}
                  </div>
                )}
                {errors.oauth && <p className="text-sm text-destructive">{errors.oauth}</p>}

                {authMode === 'bearer' && (
                  <div className="space-y-4 rounded-aurora-2 border border-aurora-border-strong bg-aurora-control-surface/70 p-4 shadow-[var(--aurora-highlight-medium)]">
                    <RadioGroup value={authSource} onValueChange={(value) => setAuthSource(value as GatewayAuthSource)}>
                      <label
                        className={cn(
                          'flex cursor-pointer items-start gap-3 rounded-aurora-1 border p-3 transition-[border-color,background-color,box-shadow]',
                          authSource === 'paste'
                            ? 'border-aurora-accent-primary/45 bg-aurora-accent-primary/10 shadow-aurora-active-glow'
                            : 'border-aurora-border-default bg-aurora-panel-medium/60 hover:border-aurora-accent-primary/30',
                        )}
                        htmlFor="auth-source-paste"
                      >
                        <RadioGroupItem value="paste" id="auth-source-paste" />
                        <div className="space-y-1">
                          <span className="font-medium text-sm">Paste token</span>
                          <p className="text-sm text-aurora-text-muted">
                            Paste the secret here and Labby will store it in <code>~/.labby/.env</code> for you.
                          </p>
                        </div>
                      </label>
                      <label
                        className={cn(
                          'flex cursor-pointer items-start gap-3 rounded-aurora-1 border p-3 transition-[border-color,background-color,box-shadow]',
                          authSource === 'env'
                            ? 'border-aurora-accent-primary/45 bg-aurora-accent-primary/10 shadow-aurora-active-glow'
                            : 'border-aurora-border-default bg-aurora-panel-medium/60 hover:border-aurora-accent-primary/30',
                        )}
                        htmlFor="auth-source-env"
                      >
                        <RadioGroupItem value="env" id="auth-source-env" />
                        <div className="space-y-1">
                          <span className="font-medium text-sm">Use existing env var</span>
                          <p className="text-sm text-aurora-text-muted">
                            Reference an existing environment variable instead of entering a secret here.
                          </p>
                        </div>
                      </label>
                    </RadioGroup>

                    {authSource === 'paste' ? (
                      <FieldGroup>
                        <Field>
                          <FieldLabel htmlFor="bearer-token-value">Bearer token</FieldLabel>
                          <Input
                            id="bearer-token-value"
                            type="password"
                            autoComplete="new-password"
                            value={bearerTokenValue}
                            onChange={(event) => setBearerTokenValue(event.target.value)}
                            placeholder="ghp_..."
                            className={cn(gatewayInputClassName, errors.bearerTokenValue && 'border-destructive')}
                          />
                          {errors.bearerTokenValue ? (
                            <p className="text-sm text-destructive">{errors.bearerTokenValue}</p>
                          ) : (
                            <FieldDescription>
                              Paste the token only. Labby will add the <code>Bearer</code> prefix automatically if needed.
                            </FieldDescription>
                          )}
                        </Field>
                        <details className="group">
                          <summary className="flex cursor-pointer select-none list-none items-center gap-1 text-sm text-aurora-text-muted [&::-webkit-details-marker]:hidden">
                            <ChevronRight className="size-3 transition-transform group-open:rotate-90" />
                            Advanced
                          </summary>
                          <div className="mt-3">
                            <Field>
                              <FieldLabel htmlFor="bearer-token-env-override">Env var name</FieldLabel>
                              <Input
                                id="bearer-token-env-override"
                                value={bearerTokenEnv}
                                onChange={(event) => setBearerTokenEnv(event.target.value)}
                                placeholder={defaultGatewayBearerEnvName(name || 'gateway')}
                                className={cn(gatewayInputClassName, errors.bearerTokenEnv && 'border-destructive')}
                              />
                              {errors.bearerTokenEnv ? (
                                <p className="text-sm text-destructive">{errors.bearerTokenEnv}</p>
                              ) : (
                                <FieldDescription>
                                  Optional. Leave blank to let Labby generate an env var name automatically.
                                </FieldDescription>
                              )}
                            </Field>
                          </div>
                        </details>
                      </FieldGroup>
                    ) : (
                      <Field>
                        <FieldLabel htmlFor="bearer-token-env">Bearer token env var</FieldLabel>
                        <Input
                          id="bearer-token-env"
                          value={bearerTokenEnv}
                          onChange={(event) => setBearerTokenEnv(event.target.value)}
                          placeholder={defaultGatewayBearerEnvName(name || 'gateway')}
                          className={cn(gatewayInputClassName, errors.bearerTokenEnv && 'border-destructive')}
                        />
                        {errors.bearerTokenEnv ? (
                          <p className="text-sm text-destructive">{errors.bearerTokenEnv}</p>
                        ) : (
                          <FieldDescription>
                            Enter the env var name only. The env var value can be a bare token or a full <code>Bearer ...</code> header.
                          </FieldDescription>
                        )}
                      </Field>
                    )}
                  </div>
                )}
              </div>
            </details>
            )}

            <details className="group order-4 rounded-aurora-2 border border-aurora-border-strong bg-aurora-control-surface/70 p-4 shadow-[var(--aurora-highlight-medium)]">
              <summary className="flex cursor-pointer select-none list-none items-center gap-2 text-sm font-semibold text-aurora-text-primary [&::-webkit-details-marker]:hidden">
                <ChevronRight className="size-4 transition-transform group-open:rotate-90" />
                <Settings2 className="size-4 text-aurora-accent-primary" />
                Advanced
              </summary>
              <div className="mt-4 space-y-4">
                <div className="flex items-center justify-between gap-4">
                  <div className="space-y-0.5">
                    <Label htmlFor="proxy-resources" className="font-medium">
                      Proxy resources
                    </Label>
                    <p className="text-sm text-aurora-text-muted">
                      Forward MCP resource requests to this server
                    </p>
                  </div>
                  <Switch
                    id="proxy-resources"
                    checked={proxyResources}
                    onCheckedChange={setProxyResources}
                  />
                </div>

                <div className="flex items-center justify-between gap-4">
                  <div className="space-y-0.5">
                    <Label htmlFor="proxy-prompts" className="font-medium">
                      Proxy prompts
                    </Label>
                    <p className="text-sm text-aurora-text-muted">
                      Forward MCP prompt requests to this server
                    </p>
                  </div>
                  <Switch
                    id="proxy-prompts"
                    checked={proxyPrompts}
                    onCheckedChange={setProxyPrompts}
                  />
                </div>

                <div className="flex items-center justify-between gap-4">
                  <div className="space-y-0.5">
                    <Label htmlFor="proxy-mcp-ui" className="font-medium">
                      Proxy MCP-UI
                    </Label>
                    <p className="text-sm text-aurora-text-muted">
                      Forward MCP-UI resources through this gateway when available
                    </p>
                  </div>
                  <Switch
                    id="proxy-mcp-ui"
                    checked={proxyMcpUi}
                    onCheckedChange={setProxyMcpUi}
                  />
                </div>
              </div>
            </details>
          </TabsContent>
        </Tabs>
        </div>

        {/* JSON drawer */}
        <div
          data-gateway-json-drawer
          className={cn(
            'absolute top-0 bottom-0 bg-aurora-page-bg border-l border-aurora-border-strong rounded-r-lg overflow-hidden transition-[width] duration-[250ms] ease-[cubic-bezier(.4,0,.2,1)] flex flex-col sm:left-full',
            'max-[600px]:fixed max-[600px]:inset-0 max-[600px]:rounded-none max-[600px]:border-l-0 max-[600px]:z-50',
            jsonDrawerOpen
              ? 'max-[600px]:h-full'
              : 'w-0',
          )}
          style={{ width: jsonDrawerOpen ? 'min(480px, 100vw)' : '0px' }}
          aria-hidden={!jsonDrawerOpen}
        >
          <div className="flex shrink-0 items-start justify-between gap-3 border-b border-aurora-border-strong bg-aurora-panel-strong px-5 py-4 shadow-[var(--aurora-shadow-medium),var(--aurora-highlight-medium)]">
            <div className="flex min-w-0 items-start gap-3">
              <div className="flex size-9 shrink-0 items-center justify-center rounded-aurora-1 border border-aurora-accent-primary/35 bg-aurora-accent-primary/10 text-aurora-accent-primary shadow-[var(--aurora-highlight-medium)]">
                <FileJson2 className="size-4" />
              </div>
              <div className="min-w-0 space-y-1">
                <p className="text-[13px] font-semibold text-aurora-text-primary">
                  JSON config
                </p>
                <p className="text-[12px] leading-5 text-aurora-text-muted">
                  Paste a client config or tune the generated server spec.
                </p>
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <span className={cn('rounded-full border px-2 py-0.5 text-[11px] font-semibold', jsonStatusClassName)}>
                {jsonStatusLabel}
              </span>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="size-8 max-[600px]:inline-flex sm:hidden"
                onClick={() => setJsonDrawerOpen(false)}
                aria-label="Close JSON editor"
              >
                <X className="size-4" />
              </Button>
            </div>
          </div>

          <div className="flex flex-1 flex-col gap-4 overflow-y-auto bg-aurora-panel-medium/30 p-5 aurora-scrollbar">
            <div className="rounded-aurora-2 border border-aurora-border-strong bg-aurora-control-surface/75 p-4 shadow-[var(--aurora-highlight-medium)]">
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0 space-y-1">
                  <p className="text-[12px] font-semibold text-aurora-text-primary">Live sync</p>
                  <p className="text-[12px] leading-5 text-aurora-text-muted">
                    Form changes regenerate JSON. Valid JSON updates the form fields.
                  </p>
                </div>
                <span className="shrink-0 rounded-full border border-aurora-border-strong bg-aurora-page-bg/80 px-2 py-0.5 text-[11px] font-semibold text-aurora-text-muted">
                  Two-way
                </span>
              </div>
              <div className="mt-3 flex flex-wrap gap-1.5">
                <span className="rounded-full border border-aurora-accent-primary/30 bg-aurora-accent-primary/10 px-2 py-0.5 text-xs font-medium text-aurora-accent-primary">
                  {name.trim() || 'Unnamed'}
                </span>
                <span className="rounded-full border border-aurora-border-strong bg-aurora-page-bg/80 px-2 py-0.5 text-xs font-medium text-aurora-text-muted">
                  {jsonTransportLabel}
                </span>
                {Object.keys(stdioEnv).length > 0 && (
                <span className="rounded-full border border-aurora-border-strong bg-aurora-page-bg/80 px-2 py-0.5 text-xs font-medium text-aurora-text-muted">
                  {Object.keys(stdioEnv).length} env vars
                </span>
                )}
              </div>
            </div>

            <GatewayConfigEditor
              value={jsonText}
              hasText={jsonHasText}
              onChange={(next) => {
                setJsonText(next)
                parseJsonToForm(next)
              }}
              onCopy={() => {
                void navigator.clipboard.writeText(jsonText)
              }}
            />

            {!jsonValid && jsonHasText && (
              <div className="flex items-start gap-2 rounded-aurora-1 border border-aurora-error/35 bg-aurora-error-surface px-3 py-2 text-[12px] leading-5 text-aurora-error">
                <AlertCircle className="mt-0.5 size-4 shrink-0" />
                Use one server entry, either directly or inside <code>mcpServers</code>.
              </div>
            )}

            <details className="group rounded-aurora-2 border border-aurora-border-default bg-aurora-control-surface/60 p-3 shadow-[var(--aurora-highlight-medium)]">
              <summary className="flex cursor-pointer select-none list-none items-center justify-between gap-3 text-[12px] font-semibold text-aurora-text-primary [&::-webkit-details-marker]:hidden">
                <span className="flex items-center gap-2">
                  <ChevronRight className="size-3.5 transition-transform group-open:rotate-90" />
                  Accepted JSON shapes
                </span>
                <span className="text-[11px] text-aurora-text-muted">Examples</span>
              </summary>
              <div className="mt-3 grid gap-2 text-[12px] leading-5 text-aurora-text-muted">
                <code className="block rounded-aurora-1 border border-aurora-border-strong bg-aurora-page-bg/80 px-2 py-1 text-aurora-text-primary">
                  {'{ "my-server": { "url": "https://..." } }'}
                </code>
                <code className="block rounded-aurora-1 border border-aurora-border-strong bg-aurora-page-bg/80 px-2 py-1 text-aurora-text-primary">
                  {'{ "mcpServers": { "local": { "command": "npx", "args": ["..."], "env": {} } } }'}
                </code>
              </div>
            </details>
          </div>
          <div className="flex gap-2 border-t border-aurora-border-strong bg-aurora-panel-strong p-3 shadow-[var(--aurora-highlight-medium)]">
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="w-full justify-center gap-2 border-aurora-border-strong bg-aurora-control-surface text-aurora-text-primary hover:bg-aurora-hover-bg"
              onClick={async () => {
                try {
                  const text = await navigator.clipboard.readText()
                  setJsonText(text)
                  parseJsonToForm(text)
                } catch {
                  // clipboard access denied
                }
              }}
            >
              <ClipboardPaste className="size-4" />
              Paste
            </Button>
          </div>
        </div>

        {saveError && (
          <div className="shrink-0 flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            <AlertCircle className="size-4 mt-0.5 shrink-0" />
            <span>{saveError}</span>
          </div>
        )}
        <DialogFooter className="shrink-0 gap-2 sm:gap-0">
          {isEditing && !isLabGateway && (
            <Button
              type="button"
              variant="outline"
              onClick={handleTest}
              disabled={isTesting || isSaving}
              className="mr-auto"
            >
              {isTesting ? (
                <Loader2 className="size-4 mr-2 animate-spin" />
              ) : (
                <Play className="size-4 mr-2" />
              )}
              Test
            </Button>
          )}
          <Button variant="outline" onClick={() => requestOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={handleSave} disabled={isSaving || isTesting}>
            {isSaving && <Loader2 className="size-4 mr-2 animate-spin" />}
            {mode === 'lab'
              ? isEditing
                ? 'Save service'
                : 'Configure service'
              : isEditing
                ? 'Save changes'
                : 'Add server'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
