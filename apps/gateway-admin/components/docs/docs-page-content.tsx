'use client'

import { useCallback, useEffect, useMemo, useState } from 'react'
import { BookOpenText, FileText, Search } from 'lucide-react'
import { useRouter, useSearchParams } from 'next/navigation'

import { AppHeader } from '@/components/app-header'
import { AURORA_DISPLAY_2, AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { SafeMarkdown } from '@/components/markdown/safe-markdown'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import {
  encodeDocAssetPath,
  parseDocsManifest,
  resolveDocHref,
  type EmbeddedDoc,
  type EmbeddedDocsManifest,
} from '@/lib/docs/catalog'
import { cn } from '@/lib/utils'

function formatBytes(bytes: number) {
  if (bytes < 1024) return bytes + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KiB'
  return (bytes / (1024 * 1024)).toFixed(1) + ' MiB'
}

function compareSections(a: string, b: string) {
  if (a === 'Overview') return -1
  if (b === 'Overview') return 1
  return a.localeCompare(b)
}

function DocsNavigation({
  documents,
  selectedPath,
  onSelect,
}: {
  documents: EmbeddedDoc[]
  selectedPath: string | null
  onSelect: (path: string) => void
}) {
  const [query, setQuery] = useState('')
  const groupedDocuments = useMemo(() => {
    const normalizedQuery = query.trim().toLowerCase()
    const groups = new Map<string, EmbeddedDoc[]>()

    for (const doc of documents) {
      if (
        normalizedQuery &&
        !doc.title.toLowerCase().includes(normalizedQuery) &&
        !doc.path.toLowerCase().includes(normalizedQuery) &&
        !doc.status?.toLowerCase().includes(normalizedQuery)
      ) {
        continue
      }

      const group = groups.get(doc.section) ?? []
      group.push(doc)
      groups.set(doc.section, group)
    }

    return [...groups.entries()].sort(([a], [b]) => compareSections(a, b))
  }, [documents, query])

  return (
    <aside className="min-w-0 rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium lg:sticky lg:top-0 lg:max-h-[calc(100dvh-9rem)]">
      <div className="border-b border-aurora-border-subtle p-4">
        <div className="flex items-center gap-3">
          <div className="flex size-9 items-center justify-center rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface text-aurora-accent-primary">
            <BookOpenText className="size-4" />
          </div>
          <div className="min-w-0">
            <p className={cn(AURORA_DISPLAY_2, 'text-[16px] leading-5')}>Documentation</p>
            <p className="text-[12px] text-aurora-text-muted">
              {documents.length} embedded documents
            </p>
          </div>
        </div>
        <div className="relative mt-4">
          <Search className="pointer-events-none absolute left-3 top-1/2 size-3.5 -translate-y-1/2 text-aurora-text-dim" />
          <Input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search docs"
            className="h-9 pl-9 text-[13px]"
            aria-label="Search documentation"
          />
        </div>
      </div>

      <nav className="max-h-[42dvh] overflow-y-auto p-2 lg:max-h-[calc(100dvh-16.5rem)]" aria-label="Documentation files">
        {groupedDocuments.length === 0 ? (
          <p className="px-2 py-6 text-center text-[13px] text-aurora-text-muted">
            No documentation matches that search.
          </p>
        ) : (
          groupedDocuments.map(([section, docs]) => (
            <div key={section} className="mb-4 last:mb-0">
              <p className={cn(AURORA_MUTED_LABEL, 'px-2 pb-1.5 pt-1')}>{section}</p>
              <div className="space-y-0.5">
                {docs.map((doc) => {
                  const selected = doc.path === selectedPath
                  return (
                    <button
                      key={doc.path}
                      type="button"
                      onClick={() => onSelect(doc.path)}
                      aria-current={selected ? 'page' : undefined}
                      className={cn(
                        'flex w-full min-w-0 items-start gap-2 rounded-aurora-1 px-2.5 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/35',
                        selected
                          ? 'bg-aurora-accent-soft text-aurora-text-primary'
                          : 'text-aurora-text-muted hover:bg-aurora-control-surface hover:text-aurora-text-primary',
                      )}
                    >
                      <FileText className="mt-0.5 size-3.5 shrink-0" />
                      <span className="min-w-0">
                        <span className="flex min-w-0 items-center gap-1.5">
                          <span className="min-w-0 flex-1 truncate text-[13px] font-medium">{doc.title}</span>
                          {doc.status ? (
                            <Badge variant="outline" className="h-[18px] max-w-28 shrink-0 truncate px-1.5 text-[9px] font-medium">
                              {doc.status}
                            </Badge>
                          ) : null}
                        </span>
                        <span className="mt-0.5 block truncate font-mono text-[10px] text-aurora-text-dim">
                          {doc.path}
                        </span>
                      </span>
                    </button>
                  )
                })}
              </div>
            </div>
          ))
        )}
      </nav>
    </aside>
  )
}

export function DocsPageContent() {
  const router = useRouter()
  const searchParams = useSearchParams()
  const requestedPath = searchParams.get('doc')
  const [manifest, setManifest] = useState<EmbeddedDocsManifest | null>(null)
  const [manifestError, setManifestError] = useState<string | null>(null)
  const [markdown, setMarkdown] = useState<string | null>(null)
  const [documentError, setDocumentError] = useState<string | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    setManifestError(null)

    fetch('/_docs/manifest.json', { signal: controller.signal })
      .then(async (response) => {
        if (!response.ok) throw new Error('Documentation manifest returned ' + response.status)
        const parsed = parseDocsManifest(await response.json())
        if (!parsed) throw new Error('Documentation manifest has an unsupported format')
        setManifest(parsed)
      })
      .catch((error: unknown) => {
        if (error instanceof DOMException && error.name === 'AbortError') return
        setManifestError(error instanceof Error ? error.message : 'Unable to load documentation manifest')
      })

    return () => controller.abort()
  }, [])

  const selectedDoc = useMemo(() => {
    if (!manifest || manifest.documents.length === 0) return null
    if (requestedPath) {
      const requested = manifest.documents.find((doc) => doc.path === requestedPath)
      if (requested) return requested
    }
    return manifest.documents.find((doc) => doc.path === 'README.md') ?? manifest.documents[0]
  }, [manifest, requestedPath])

  const knownPaths = useMemo(
    () => new Set(manifest?.documents.map((doc) => doc.path) ?? []),
    [manifest],
  )

  useEffect(() => {
    if (!selectedDoc) {
      setMarkdown(null)
      return
    }

    const controller = new AbortController()
    setMarkdown(null)
    setDocumentError(null)

    fetch('/_docs/' + encodeDocAssetPath(selectedDoc.path), { signal: controller.signal })
      .then(async (response) => {
        if (!response.ok) throw new Error('Documentation file returned ' + response.status)
        setMarkdown(await response.text())
      })
      .catch((error: unknown) => {
        if (error instanceof DOMException && error.name === 'AbortError') return
        setDocumentError(error instanceof Error ? error.message : 'Unable to load documentation file')
      })

    return () => controller.abort()
  }, [selectedDoc])

  const selectDocument = useCallback(
    (path: string) => {
      router.push('/docs?doc=' + encodeURIComponent(path))
    },
    [router],
  )

  const transformDocUrl = useCallback(
    (url: string) => selectedDoc ? resolveDocHref(selectedDoc.path, url, knownPaths) : url,
    [knownPaths, selectedDoc],
  )

  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'Documentation' }]} />
      {manifestError ? (
        <Alert variant="error">
          <AlertTitle>Documentation unavailable</AlertTitle>
          <AlertDescription>{manifestError}</AlertDescription>
        </Alert>
      ) : !manifest ? (
        <div className="grid gap-4 lg:grid-cols-[280px_minmax(0,1fr)]">
          <Skeleton className="h-[520px] w-full rounded-aurora-2" />
          <Skeleton className="h-[620px] w-full rounded-aurora-2" />
        </div>
      ) : (
        <div className="grid min-w-0 gap-4 lg:grid-cols-[300px_minmax(0,1fr)]">
          <DocsNavigation
            documents={manifest.documents}
            selectedPath={selectedDoc?.path ?? null}
            onSelect={selectDocument}
          />

          <main className="min-w-0 rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-strong">
            {selectedDoc ? (
              <>
                <div className="flex flex-wrap items-center justify-between gap-2 border-b border-aurora-border-subtle px-5 py-3 sm:px-7">
                  <div className="flex min-w-0 items-center gap-2">
                    <p className="min-w-0 truncate font-mono text-[11px] text-aurora-text-dim">
                      {selectedDoc.path}
                    </p>
                    {selectedDoc.status ? (
                      <Badge variant="outline" className="max-w-44 shrink-0 truncate text-[10px]">
                        {selectedDoc.status}
                      </Badge>
                    ) : null}
                  </div>
                  <p className="shrink-0 text-[11px] text-aurora-text-dim">
                    {formatBytes(selectedDoc.bytes)} · embedded at build time
                  </p>
                </div>
                <div className="mx-auto max-w-[1040px] px-5 py-6 sm:px-8 sm:py-8 lg:px-10">
                  {selectedDoc.status?.startsWith('historical') ? (
                    <Alert className="mb-5">
                      <AlertTitle>Historical implementation record</AlertTitle>
                      <AlertDescription>
                        This document is labeled <code>{selectedDoc.status}</code>. Use the linked canonical
                        service, runtime, or generated documentation for current product behavior.
                      </AlertDescription>
                    </Alert>
                  ) : null}
                  {documentError ? (
                    <Alert variant="error">
                      <AlertTitle>Unable to load document</AlertTitle>
                      <AlertDescription>{documentError}</AlertDescription>
                    </Alert>
                  ) : markdown === null ? (
                    <div className="space-y-4">
                      <Skeleton className="h-9 w-2/3" />
                      <Skeleton className="h-4 w-full" />
                      <Skeleton className="h-4 w-5/6" />
                      <Skeleton className="h-36 w-full" />
                    </div>
                  ) : (
                    <SafeMarkdown
                      text={markdown}
                      transformUrl={transformDocUrl}
                      className="text-[14px] leading-[1.7] [&_h1]:mb-5 [&_h1]:text-[30px] [&_h1]:font-semibold [&_h1]:tracking-[-0.03em] [&_h2]:mb-3 [&_h2]:mt-8 [&_h2]:text-[22px] [&_h2]:font-semibold [&_h3]:mb-2 [&_h3]:mt-6 [&_h3]:text-[17px] [&_p]:my-3 [&_pre]:my-4 [&_table]:my-4"
                    />
                  )}
                </div>
              </>
            ) : (
              <div className="p-8 text-[14px] text-aurora-text-muted">
                No embedded documentation was found in this build.
              </div>
            )}
          </main>
        </div>
      )}
    </>
  )
}
