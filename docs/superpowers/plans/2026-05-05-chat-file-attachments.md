# Chat File Attachments Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add local-device file attachments to the Gateway Admin chat prompt while preserving existing workspace-file attachments and ACP prompt behavior.

**Architecture:** Keep browser-selected files as a distinct attachment variant from workspace paths. The frontend validates, previews, removes, and serializes local files before send; the Rust ACP HTTP/dispatch/runtime path validates the payload again and emits ACP `ContentBlock::Resource` blocks for supported providers. The text prompt remains the first content block, and attachments never get logged as raw bytes.

**Tech Stack:** Next.js static export, React 19, TypeScript, node:test, Playwright browser tests, Rust axum API, `agent-client-protocol` Rust crate, `cargo nextest`.

---

## Scope And Existing Facts

Existing chat already supports workspace-file attachments:

- `apps/gateway-admin/components/chat/chat-input.tsx` keeps `attachments: AttachmentRef[]`, opens `WorkspacePicker`, renders `AttachmentChip`, and sends `{ text, attachments }`.
- `apps/gateway-admin/lib/fs/types.ts` defines `AttachmentRef = { kind: 'file'; path: string }`.
- `apps/gateway-admin/lib/chat/use-chat-session-controller.ts` posts `attachments` to `/v1/acp/sessions/{runId}/prompt`, but the Rust handler currently ignores that field because `PromptBody` only has `prompt` and `page_context`.
- `crates/lab/src/api/services/acp.rs` validates prompt text and passes `"text"` to `dispatch/acp`.
- `crates/lab/src/dispatch/acp/dispatch.rs` builds a string prompt with optional page context and calls `registry.prompt_session(session_id, &effective_text, principal)`.
- `crates/lab/src/acp/registry.rs` and `crates/lab/src/acp/runtime.rs` carry only `String` prompts today.
- `crates/lab/src/acp/runtime.rs` currently calls `session.send_prompt(prompt)`, which constructs a text-only `PromptRequest`.
- The ACP schema supports `PromptRequest { prompt: Array<ContentBlock> }`, with text, image, resource links, and embedded resources. Embedded resource content supports text or base64 blob contents.
- The Rust ACP bridge currently advertises provider filesystem reads/writes as disabled. Do not implement local-file attachment by turning provider filesystem reads on.

Best-practice constraints:

- Use `<input type="file" multiple>` for desktop and mobile file pickers. `multiple` allows one or more files on file inputs.
- Use `accept` only as picker guidance; still validate MIME/type/size in code.
- Use object URLs only for local image previews and revoke them after removal/unmount.
- Avoid `readAsDataURL` for transport because it adds a `data:*/*;base64,` prefix that must be stripped and inflates memory. Prefer `arrayBuffer()` plus explicit base64 conversion for binary payloads and `text()` for text payloads.

## Chosen Product Scope

Ship local-device attachments in the existing chat input. Do not add drag-and-drop, directory upload, resumable upload, server-side attachment persistence, virus scanning, or cloud drive providers in this bead.

Limits for the first implementation:

- `MAX_LOCAL_ATTACHMENTS = 5`
- `MAX_LOCAL_ATTACHMENT_BYTES = 2 * 1024 * 1024`
- allowed MIME prefixes/types:
  - `text/*`
  - `application/json`
  - `application/pdf`
  - `image/png`
  - `image/jpeg`
  - `image/gif`
  - `image/webp`

These constants belong in frontend and backend code so both surfaces fail the same inputs. If product wants a different limit, update the constants and tests together.

## File Map

- Modify `apps/gateway-admin/lib/fs/types.ts`: extend attachment types with a local-device variant and serializable prompt payload metadata.
- Create `apps/gateway-admin/lib/chat/local-attachments.ts`: frontend validation, stable IDs, preview URL ownership, text/binary serialization helpers.
- Create `apps/gateway-admin/lib/chat/local-attachments.test.ts`: unit tests for success, oversize, unsupported type, max count, text serialization, binary base64 serialization.
- Modify `apps/gateway-admin/components/chat/chat-input.tsx`: add hidden file input, local attach button behavior, local preview chips, removal, validation errors, send blocking while files serialize.
- Modify `apps/gateway-admin/components/chat/chat-shell.test.tsx`: add controller-level assertion that local attachment payloads are posted without dropping prompt text.
- Modify `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`: cover local file picker, visible chip, removal, and posted payload.
- Modify `crates/lab/src/api/services/acp.rs`: accept `attachments` in `PromptBody`, validate HTTP size/count/type, and pass sanitized attachment params to dispatch.
- Modify `crates/lab/src/dispatch/acp/params.rs`: add attachment param structs/parsers and shared backend limits.
- Modify `crates/lab/src/dispatch/acp/dispatch.rs`: preserve page-context text behavior and build a structured prompt input containing text plus attachments.
- Modify `crates/lab/src/acp/registry.rs`: add `prompt_session_with_attachments` or replace the internal prompt input with a struct while keeping existing `prompt_session` text-only callers working.
- Modify `crates/lab/src/acp/runtime.rs`: add a command variant for structured content blocks and send `PromptRequest::new(session_id, content_blocks)` directly rather than using text-only `session.send_prompt`.
- Modify `crates/lab/tests/acp_backend_contract.rs`: assert the fake provider receives text plus resource blocks and rejects invalid attachment payloads.
- Optionally modify `apps/gateway-admin/components/chat/message-bubble.tsx`: show sent user-message attachment metadata once events preserve attachments. Keep this optional unless backend events add attachment metadata in the same task.

## Data Flow

```text
Browser FileList
  -> validate count/type/size
  -> local preview chips
  -> on send: text() or arrayBuffer() -> serializable attachments
  -> POST /v1/acp/sessions/{id}/prompt
  -> Rust HTTP validation
  -> dispatch page-context prefix + attachment conversion
  -> AcpSessionRegistry ownership/state checks
  -> RuntimeHandle structured prompt command
  -> ACP PromptRequest [TextContent, EmbeddedResource...]
```

Failure paths that must be visible:

- Too many files: inline input error, no network request.
- Oversized file: inline input error naming the file and limit, no network request.
- Unsupported file type: inline input error naming the file/type, no network request.
- Browser read failure: inline input error, no network request.
- Backend validation rejection: toast/status path already used for prompt failure; message must name `invalid_param` cause without echoing base64.
- Provider lacks embedded context: backend returns a user-visible error instead of silently sending text only.

## Task 1: Frontend Attachment Model And Serialization

**Files:**
- Modify: `apps/gateway-admin/lib/fs/types.ts`
- Create: `apps/gateway-admin/lib/chat/local-attachments.ts`
- Create: `apps/gateway-admin/lib/chat/local-attachments.test.ts`

- [ ] **Step 1: Write failing tests for frontend validation and serialization**

Add `apps/gateway-admin/lib/chat/local-attachments.test.ts`:

```ts
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  MAX_LOCAL_ATTACHMENTS,
  MAX_LOCAL_ATTACHMENT_BYTES,
  fileToSerializableAttachment,
  validateLocalFiles,
} from './local-attachments.ts'

function file(name: string, type: string, body: string): File {
  return new File([body], name, { type })
}

test('validateLocalFiles accepts supported files within count and size limits', () => {
  const files = [
    file('notes.txt', 'text/plain', 'hello'),
    file('diagram.png', 'image/png', 'png-bytes'),
  ]

  const result = validateLocalFiles(files, [])

  assert.deepEqual(result.errors, [])
  assert.equal(result.accepted.length, 2)
})

test('validateLocalFiles rejects unsupported types, oversized files, and count overflow', () => {
  const existing = Array.from({ length: MAX_LOCAL_ATTACHMENTS }, (_, index) =>
    file(`existing-${index}.txt`, 'text/plain', 'x'),
  )
  const unsupported = file('archive.zip', 'application/zip', 'zip')
  const oversized = new File([new Uint8Array(MAX_LOCAL_ATTACHMENT_BYTES + 1)], 'big.txt', {
    type: 'text/plain',
  })

  const result = validateLocalFiles([unsupported, oversized], existing)

  assert.equal(result.accepted.length, 0)
  assert.ok(result.errors.some((message) => message.includes('You can attach up to 5 files')))
  assert.ok(result.errors.some((message) => message.includes('archive.zip has unsupported type application/zip')))
  assert.ok(result.errors.some((message) => message.includes('big.txt is larger than 2 MiB')))
})

test('fileToSerializableAttachment emits text resources for text files', async () => {
  const result = await fileToSerializableAttachment(
    {
      id: 'local-1',
      kind: 'local',
      file: file('notes.txt', 'text/plain', 'hello world'),
      previewUrl: null,
    },
  )

  assert.deepEqual(result, {
    kind: 'local',
    id: 'local-1',
    name: 'notes.txt',
    mimeType: 'text/plain',
    size: 11,
    contentKind: 'text',
    text: 'hello world',
  })
})

test('fileToSerializableAttachment emits base64 blob resources for binary files', async () => {
  const result = await fileToSerializableAttachment(
    {
      id: 'local-2',
      kind: 'local',
      file: new File([new Uint8Array([1, 2, 3])], 'image.png', { type: 'image/png' }),
      previewUrl: 'blob:http://localhost/image',
    },
  )

  assert.deepEqual(result, {
    kind: 'local',
    id: 'local-2',
    name: 'image.png',
    mimeType: 'image/png',
    size: 3,
    contentKind: 'blob',
    base64: 'AQID',
  })
})
```

- [ ] **Step 2: Run tests and confirm they fail because the module does not exist**

Run:

```bash
cd apps/gateway-admin
pnpm exec tsx --test lib/chat/local-attachments.test.ts
```

Expected: FAIL with a module-not-found error for `./local-attachments.ts`.

- [ ] **Step 3: Extend attachment types**

In `apps/gateway-admin/lib/fs/types.ts`, replace the current `AttachmentRef` definition with:

```ts
export type WorkspaceAttachmentRef = { kind: 'file'; path: string }

export type LocalAttachmentDraft = {
  kind: 'local'
  id: string
  file: File
  previewUrl: string | null
}

export type SerializableLocalAttachment =
  | {
      kind: 'local'
      id: string
      name: string
      mimeType: string
      size: number
      contentKind: 'text'
      text: string
    }
  | {
      kind: 'local'
      id: string
      name: string
      mimeType: string
      size: number
      contentKind: 'blob'
      base64: string
    }

export type PromptAttachmentRef = WorkspaceAttachmentRef | SerializableLocalAttachment
export type AttachmentRef = WorkspaceAttachmentRef | LocalAttachmentDraft
```

- [ ] **Step 4: Implement validation and serialization**

Create `apps/gateway-admin/lib/chat/local-attachments.ts`:

```ts
import type { LocalAttachmentDraft, SerializableLocalAttachment } from '@/lib/fs/types'

export const MAX_LOCAL_ATTACHMENTS = 5
export const MAX_LOCAL_ATTACHMENT_BYTES = 2 * 1024 * 1024

const ALLOWED_EXACT_TYPES = new Set([
  'application/json',
  'application/pdf',
  'image/png',
  'image/jpeg',
  'image/gif',
  'image/webp',
])

export function isSupportedLocalAttachmentType(mimeType: string): boolean {
  const normalized = mimeType.trim().toLowerCase()
  return normalized.startsWith('text/') || ALLOWED_EXACT_TYPES.has(normalized)
}

export function localAttachmentId(file: File): string {
  return `local-${file.name}-${file.size}-${file.lastModified}`
}

export function createLocalAttachmentDraft(file: File): LocalAttachmentDraft {
  const mimeType = file.type.trim().toLowerCase()
  const previewUrl = mimeType.startsWith('image/') ? URL.createObjectURL(file) : null
  return {
    kind: 'local',
    id: localAttachmentId(file),
    file,
    previewUrl,
  }
}

export function revokeLocalAttachmentPreview(attachment: LocalAttachmentDraft): void {
  if (attachment.previewUrl) {
    URL.revokeObjectURL(attachment.previewUrl)
  }
}

export function validateLocalFiles(
  incoming: File[],
  existingLocalAttachments: readonly File[],
): { accepted: File[]; errors: string[] } {
  const errors: string[] = []
  const remainingSlots = Math.max(0, MAX_LOCAL_ATTACHMENTS - existingLocalAttachments.length)

  if (incoming.length > remainingSlots) {
    errors.push(`You can attach up to ${MAX_LOCAL_ATTACHMENTS} files.`)
  }

  const accepted: File[] = []
  for (const candidate of incoming.slice(0, remainingSlots)) {
    const mimeType = candidate.type || 'application/octet-stream'
    if (!isSupportedLocalAttachmentType(mimeType)) {
      errors.push(`${candidate.name} has unsupported type ${mimeType}.`)
      continue
    }

    if (candidate.size > MAX_LOCAL_ATTACHMENT_BYTES) {
      errors.push(`${candidate.name} is larger than 2 MiB.`)
      continue
    }

    accepted.push(candidate)
  }

  return { accepted, errors }
}

function arrayBufferToBase64(buffer: ArrayBuffer): string {
  let binary = ''
  const bytes = new Uint8Array(buffer)
  const chunkSize = 0x8000
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize))
  }
  return btoa(binary)
}

export async function fileToSerializableAttachment(
  attachment: LocalAttachmentDraft,
): Promise<SerializableLocalAttachment> {
  const mimeType = attachment.file.type || 'application/octet-stream'
  const base = {
    kind: 'local' as const,
    id: attachment.id,
    name: attachment.file.name,
    mimeType,
    size: attachment.file.size,
  }

  if (mimeType.trim().toLowerCase().startsWith('text/') || mimeType === 'application/json') {
    return {
      ...base,
      contentKind: 'text',
      text: await attachment.file.text(),
    }
  }

  return {
    ...base,
    contentKind: 'blob',
    base64: arrayBufferToBase64(await attachment.file.arrayBuffer()),
  }
}
```

- [ ] **Step 5: Run focused frontend tests**

Run:

```bash
cd apps/gateway-admin
pnpm exec tsx --test lib/chat/local-attachments.test.ts
```

Expected: PASS.

## Task 2: Chat Input UI For Local Device Files

**Files:**
- Modify: `apps/gateway-admin/components/chat/chat-input.tsx`
- Modify: `apps/gateway-admin/components/chat/chat-shell.test.tsx`

- [ ] **Step 1: Write failing UI test coverage**

In `apps/gateway-admin/components/chat/chat-shell.test.tsx`, add a focused test that exercises the controller payload shape without rendering browser-only file inputs:

```ts
test('sendPromptForSelectedProvider posts local attachment payloads without dropping prompt text', async () => {
  const requests: Array<{ path: string; body: unknown }> = []

  await sendPromptForSelectedProvider({
    payload: {
      text: 'summarize this file',
      attachments: [
        {
          kind: 'local',
          id: 'local-notes',
          name: 'notes.txt',
          mimeType: 'text/plain',
          size: 11,
          contentKind: 'text',
          text: 'hello world',
        },
      ],
    },
    selectedRun: {
      ...run('run-codex'),
      provider: 'codex-acp',
    },
    selectedProviderId: 'codex-acp',
    createSession: async () => run('unused'),
    isMobileViewport: false,
    fetchAcp: async (path, init) => {
      requests.push({ path, body: JSON.parse(String(init?.body)) })
      return new Response(JSON.stringify({ ok: true }), { status: 200 })
    },
    refreshSessions: async () => {},
    addOptimisticMessage: () => {},
    removeOptimisticMessage: () => {},
  })

  assert.deepEqual(requests, [
    {
      path: '/sessions/run-codex/prompt',
      body: {
        prompt: 'summarize this file',
        attachments: [
          {
            kind: 'local',
            id: 'local-notes',
            name: 'notes.txt',
            mimeType: 'text/plain',
            size: 11,
            contentKind: 'text',
            text: 'hello world',
          },
        ],
      },
    },
  ])
})
```

- [ ] **Step 2: Run the focused existing test file**

Run:

```bash
cd apps/gateway-admin
pnpm exec tsx --test components/chat/chat-shell.test.tsx
```

Expected: FAIL until `PromptPayload.attachments` accepts `PromptAttachmentRef[]` instead of workspace-only attachments.

- [ ] **Step 3: Update prompt payload typing and serialization at send time**

In `apps/gateway-admin/lib/chat/use-chat-session-controller.ts`, change the prompt payload type:

```ts
import type { PromptAttachmentRef } from '@/lib/fs/types'

export type PromptPayload = {
  text: string
  attachments: PromptAttachmentRef[]
}
```

In `apps/gateway-admin/components/chat/chat-input.tsx`, import helpers:

```ts
import {
  createLocalAttachmentDraft,
  fileToSerializableAttachment,
  revokeLocalAttachmentPreview,
  validateLocalFiles,
} from '@/lib/chat/local-attachments'
import type { AttachmentRef, LocalAttachmentDraft, PromptAttachmentRef } from '@/lib/fs/types'
```

Update `ChatInputPayload`:

```ts
export interface ChatInputPayload {
  text: string
  attachments: PromptAttachmentRef[]
}
```

- [ ] **Step 4: Add hidden local file input and local attach handling**

Inside `ChatInput`, add:

```ts
const localFileInputRef = React.useRef<HTMLInputElement>(null)
const [attachmentError, setAttachmentError] = React.useState<string | null>(null)
```

Add helpers:

```ts
const localAttachments = attachments.filter((attachment): attachment is LocalAttachmentDraft => attachment.kind === 'local')

const handleLocalFiles = (fileList: FileList | null) => {
  if (!fileList) return
  const incoming = Array.from(fileList)
  const { accepted, errors } = validateLocalFiles(
    incoming,
    localAttachments.map((attachment) => attachment.file),
  )
  setAttachmentError(errors[0] ?? null)
  setAttachments((prev) => [...prev, ...accepted.map(createLocalAttachmentDraft)])
  if (localFileInputRef.current) {
    localFileInputRef.current.value = ''
  }
}
```

Update `handleSend` to serialize local files before calling `onSend`:

```ts
const serializedAttachments = await Promise.all(
  attachments.map((attachment) =>
    attachment.kind === 'local' ? fileToSerializableAttachment(attachment) : attachment,
  ),
)
await onSend({ text: trimmed, attachments: serializedAttachments })
attachments.forEach((attachment) => {
  if (attachment.kind === 'local') revokeLocalAttachmentPreview(attachment)
})
```

Update `removeAttachment` to revoke only the removed local preview:

```ts
const removeAttachment = (ref: AttachmentRef) => {
  if (ref.kind === 'local') revokeLocalAttachmentPreview(ref)
  setAttachments((prev) =>
    prev.filter((attachment) => {
      if (attachment.kind === 'local' && ref.kind === 'local') return attachment.id !== ref.id
      if (attachment.kind === 'file' && ref.kind === 'file') return attachment.path !== ref.path
      return true
    }),
  )
}
```

Add unmount cleanup:

```ts
React.useEffect(() => {
  return () => {
    attachments.forEach((attachment) => {
      if (attachment.kind === 'local') revokeLocalAttachmentPreview(attachment)
    })
  }
}, [attachments])
```

Render the hidden input near the buttons:

```tsx
<input
  ref={localFileInputRef}
  type="file"
  multiple
  accept="text/*,application/json,application/pdf,image/png,image/jpeg,image/gif,image/webp"
  className="sr-only"
  aria-label="Attach local file"
  onChange={(event) => handleLocalFiles(event.currentTarget.files)}
/>
```

Change the existing paperclip tooltip/button label to "Attach local file" and open the file input:

```tsx
onClick={() => localFileInputRef.current?.click()}
```

Keep the workspace picker available through a separate button if the product still needs it. Use an icon-only button with a tooltip "Attach workspace file" and the current `WorkspacePicker` behavior.

- [ ] **Step 5: Render local attachment chips**

Update `AttachmentChip` so it branches by `attachment.kind`.

For local image drafts, render:

```tsx
{attachment.kind === 'local' && attachment.previewUrl ? (
  <Image
    src={attachment.previewUrl}
    alt=""
    className="size-4 rounded-[2px] object-cover"
    height={16}
    width={16}
    unoptimized
  />
) : (
  <FileText className="size-3 text-aurora-text-muted" />
)}
```

For chip text:

```tsx
const label = attachment.kind === 'local' ? attachment.file.name : attachment.path
```

For error rendering below the chip list:

```tsx
{attachmentError && (
  <p role="alert" className="border-b border-aurora-error/30 px-3 py-1.5 text-[11px] text-aurora-error sm:px-4">
    {attachmentError}
  </p>
)}
```

- [ ] **Step 6: Run frontend focused tests**

Run:

```bash
cd apps/gateway-admin
pnpm exec tsx --test lib/chat/local-attachments.test.ts components/chat/chat-shell.test.tsx
```

Expected: PASS.

## Task 3: Browser Coverage For Picker, Preview, Removal, And Payload

**Files:**
- Modify: `apps/gateway-admin/lib/browser/chat-shell.browser.test.ts`

- [ ] **Step 1: Add failing browser test**

Append a browser test:

```ts
test('chat shell attaches and removes local files before sending prompt', { concurrency: false }, async (t) => {
  await startPreviewServer()

  const browser = await chromium.launch({ headless: true })
  t.after(async () => {
    await browser.close()
  })

  const page = await browser.newPage({ viewport: { width: 390, height: 844 } })
  const sessions: BrowserSession[] = []
  const promptRequests: Array<{ prompt: string; attachments?: unknown[] }> = []

  await mockAuthenticatedSession(page)
  await page.route('**/v1/acp/**', async (route) => {
    const request = route.request()
    const url = new URL(request.url())

    if (url.pathname === '/v1/acp/provider') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ provider: { provider: 'codex', ready: true, command: 'npx', args: [], message: 'ready' } }),
      })
      return
    }

    if (url.pathname === '/v1/acp/sessions' && request.method() === 'GET') {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ sessions }) })
      return
    }

    if (url.pathname === '/v1/acp/sessions' && request.method() === 'POST') {
      const created = session('session-attach', 'Attachment session')
      sessions.unshift(created)
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ session: created }) })
      return
    }

    const promptMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/prompt$/)
    if (promptMatch && request.method() === 'POST') {
      const payload = JSON.parse(request.postData() ?? '{}') as { prompt?: string; attachments?: unknown[] }
      promptRequests.push({ prompt: payload.prompt ?? '', attachments: payload.attachments })
      await route.fulfill({ status: 202, contentType: 'application/json', body: JSON.stringify({ accepted: true }) })
      return
    }

    const ticketMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/subscribe_ticket$/)
    if (ticketMatch && request.method() === 'POST') {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ ticket: 'ticket-attach' }) })
      return
    }

    const eventMatch = url.pathname.match(/^\/v1\/acp\/sessions\/([^/]+)\/events$/)
    if (eventMatch && request.method() === 'GET') {
      await route.fulfill({ status: 200, contentType: 'text/event-stream', body: '' })
      return
    }

    await route.fulfill({ status: 404, contentType: 'application/json', body: JSON.stringify({ message: `Unhandled ACP request: ${url.pathname}` }) })
  })

  await page.goto(`${BASE_URL}/chat/`, { waitUntil: 'networkidle' })
  await page.getByRole('textbox', { name: 'Message' }).fill('Use these notes')

  const chooserPromise = page.waitForEvent('filechooser')
  await page.getByRole('button', { name: 'Attach local file' }).click()
  const chooser = await chooserPromise
  await chooser.setFiles([
    {
      name: 'notes.txt',
      mimeType: 'text/plain',
      buffer: Buffer.from('local browser notes'),
    },
  ])

  await assert.doesNotReject(() => page.getByText('notes.txt').waitFor())
  await page.getByRole('button', { name: 'Remove notes.txt' }).click()
  await assert.doesNotReject(() => page.getByRole('button', { name: 'Send message' }).waitFor())

  const chooserPromise2 = page.waitForEvent('filechooser')
  await page.getByRole('button', { name: 'Attach local file' }).click()
  const chooser2 = await chooserPromise2
  await chooser2.setFiles([
    {
      name: 'notes.txt',
      mimeType: 'text/plain',
      buffer: Buffer.from('local browser notes'),
    },
  ])

  await page.getByRole('button', { name: 'Send message' }).click()
  await waitForCondition(() => promptRequests.length === 1)

  assert.deepEqual(promptRequests, [
    {
      prompt: 'Use these notes',
      attachments: [
        {
          kind: 'local',
          id: 'local-notes.txt-19-0',
          name: 'notes.txt',
          mimeType: 'text/plain',
          size: 19,
          contentKind: 'text',
          text: 'local browser notes',
        },
      ],
    },
  ])
})
```

If Playwright supplies a non-zero `lastModified`, relax only the `id` assertion by checking the prefix and stable metadata; keep all other fields exact.

- [ ] **Step 2: Run browser test and confirm it fails before UI support**

Run:

```bash
cd apps/gateway-admin
pnpm run test:browser -- lib/browser/chat-shell.browser.test.ts
```

Expected: FAIL because "Attach local file" does not exist yet.

- [ ] **Step 3: Run after Task 2 implementation**

Run:

```bash
cd apps/gateway-admin
pnpm run test:browser -- lib/browser/chat-shell.browser.test.ts
```

Expected: PASS for the new attachment test and existing chat browser tests.

## Task 4: Rust HTTP And Dispatch Validation

**Files:**
- Modify: `crates/lab/src/api/services/acp.rs`
- Modify: `crates/lab/src/dispatch/acp/params.rs`
- Modify: `crates/lab/src/dispatch/acp/dispatch.rs`
- Modify: `crates/lab/tests/acp_backend_contract.rs`

- [ ] **Step 1: Add failing Rust contract tests**

In `crates/lab/tests/acp_backend_contract.rs`, update the fake provider `session/prompt` arm to echo prompt blocks to stderr or a temp file. Use a temp file path passed through an env var:

```python
    elif method == "session/prompt":
        capture = os.environ.get("LAB_ACP_FAKE_PROMPT_CAPTURE")
        if capture:
            with open(capture, "a", encoding="utf-8") as out:
                out.write(json.dumps(params.get("prompt", [])) + "\n")
        print(json.dumps({"jsonrpc": "2.0", "id": req_id, "result": {}}), flush=True)
```

Add tests:

```rust
#[tokio::test]
async fn acp_prompt_accepts_local_text_attachment_as_embedded_resource() {
    let _guard = test_lock().lock().await;
    let _launch = install_fake_provider();
    let (app, registry) = acp_test_app();
    let session_id = create_owned_session(&registry, "alice").await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/acp/sessions/{session_id}/prompt"))
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "prompt": "Summarize",
                        "attachments": [{
                            "kind": "local",
                            "id": "local-notes",
                            "name": "notes.txt",
                            "mimeType": "text/plain",
                            "size": 11,
                            "contentKind": "text",
                            "text": "hello world"
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn acp_prompt_rejects_oversized_local_attachment_metadata() {
    let _guard = test_lock().lock().await;
    let (app, registry) = acp_test_app();
    let session_id = create_owned_session(&registry, "alice").await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/acp/sessions/{session_id}/prompt"))
                .header(header::AUTHORIZATION, "Bearer secret-token")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "prompt": "Summarize",
                        "attachments": [{
                            "kind": "local",
                            "id": "local-big",
                            "name": "big.txt",
                            "mimeType": "text/plain",
                            "size": 2_097_153,
                            "contentKind": "text",
                            "text": "too big"
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = json_body(response).await;
    assert_eq!(body["kind"], "invalid_param");
    assert_eq!(body["param"], "attachments");
}
```

- [ ] **Step 2: Run focused Rust tests and confirm failure**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features -E 'test(acp_prompt_)'
```

Expected: FAIL because `PromptBody` does not deserialize or forward attachments.

- [ ] **Step 3: Add backend attachment params**

In `crates/lab/src/dispatch/acp/params.rs`, add:

```rust
use serde::{Deserialize, Serialize};

pub const MAX_LOCAL_ATTACHMENTS: usize = 5;
pub const MAX_LOCAL_ATTACHMENT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", tag = "contentKind")]
pub enum LocalAttachmentContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "blob")]
    Blob { base64: String },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPromptAttachment {
    pub kind: String,
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size: u64,
    #[serde(flatten)]
    pub content: LocalAttachmentContent,
}

pub fn validate_local_attachments(
    attachments: &[LocalPromptAttachment],
) -> Result<(), crate::dispatch::error::ToolError> {
    if attachments.len() > MAX_LOCAL_ATTACHMENTS {
        return Err(crate::dispatch::error::ToolError::InvalidParam {
            message: format!("at most {MAX_LOCAL_ATTACHMENTS} attachments are allowed"),
            param: "attachments".into(),
        });
    }

    for attachment in attachments {
        if attachment.kind != "local" {
            return Err(crate::dispatch::error::ToolError::InvalidParam {
                message: "only local attachments are supported on the ACP prompt route".into(),
                param: "attachments".into(),
            });
        }
        if attachment.size > MAX_LOCAL_ATTACHMENT_BYTES {
            return Err(crate::dispatch::error::ToolError::InvalidParam {
                message: format!("attachment `{}` exceeds the 2 MiB limit", attachment.name),
                param: "attachments".into(),
            });
        }
        if !is_supported_attachment_mime(&attachment.mime_type) {
            return Err(crate::dispatch::error::ToolError::InvalidParam {
                message: format!("attachment `{}` has unsupported type `{}`", attachment.name, attachment.mime_type),
                param: "attachments".into(),
            });
        }
    }

    Ok(())
}

pub fn is_supported_attachment_mime(mime_type: &str) -> bool {
    let normalized = mime_type.trim().to_ascii_lowercase();
    normalized.starts_with("text/")
        || matches!(
            normalized.as_str(),
            "application/json"
                | "application/pdf"
                | "image/png"
                | "image/jpeg"
                | "image/gif"
                | "image/webp"
        )
}
```

- [ ] **Step 4: Accept and validate attachments in HTTP**

In `crates/lab/src/api/services/acp.rs`, add the attachment type import:

```rust
use crate::dispatch::acp::params::{validate_local_attachments, LocalPromptAttachment};
```

Extend `PromptBody`:

```rust
attachments: Option<Vec<LocalPromptAttachment>>,
```

Before building params:

```rust
let attachments = body.attachments.unwrap_or_default();
if let Err(error) = validate_local_attachments(&attachments) {
    return error.into_response();
}
```

Add to params:

```rust
"attachments": attachments,
```

- [ ] **Step 5: Build structured prompt input in dispatch**

In `crates/lab/src/dispatch/acp/dispatch.rs`, deserialize `attachments` from params after `effective_text` is built:

```rust
let attachments: Vec<LocalPromptAttachment> = params
    .get("attachments")
    .cloned()
    .map(serde_json::from_value)
    .transpose()
    .map_err(|error| ToolError::InvalidParam {
        message: format!("invalid attachments payload: {error}"),
        param: "attachments".into(),
    })?
    .unwrap_or_default();
validate_local_attachments(&attachments)?;

registry
    .prompt_session_with_attachments(session_id, &effective_text, attachments, principal)
    .await?;
```

Keep the existing text-only `prompt_session` as a wrapper so non-HTTP callers do not need immediate changes:

```rust
registry.prompt_session(session_id, &effective_text, principal).await?;
```

should become a call to the new wrapper internally, not duplicate logic.

- [ ] **Step 6: Run focused Rust validation tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features -E 'test(acp_prompt_)'
```

Expected: attachment validation tests now reach runtime behavior; structured provider capture may still fail until Task 5.

## Task 5: ACP Runtime Structured Prompt Blocks

**Files:**
- Modify: `crates/lab/src/acp/registry.rs`
- Modify: `crates/lab/src/acp/runtime.rs`
- Modify: `crates/lab/tests/acp_backend_contract.rs`

- [ ] **Step 1: Add a runtime command that carries content blocks**

In `crates/lab/src/acp/runtime.rs`, import these schema types:

```rust
use agent_client_protocol::schema::{
    BlobResourceContents, EmbeddedResource, EmbeddedResourceResource, PromptRequest, TextContent,
    TextResourceContents,
};
```

Add:

```rust
#[derive(Clone)]
pub struct PromptAttachment {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size: u64,
    pub content: PromptAttachmentContent,
}

#[derive(Clone)]
pub enum PromptAttachmentContent {
    Text(String),
    Blob(String),
}

#[derive(Clone)]
pub struct PromptInput {
    pub text: String,
    pub attachments: Vec<PromptAttachment>,
}
```

Change `SessionCommand::Prompt(String)` to:

```rust
SessionCommand::Prompt(PromptInput)
```

Keep `RuntimeHandle::prompt(&self, prompt: String)` as:

```rust
pub async fn prompt(&self, prompt: String) -> Result<(), String> {
    self.prompt_input(PromptInput { text: prompt, attachments: Vec::new() }).await
}

pub async fn prompt_input(&self, input: PromptInput) -> Result<(), String> {
    self.command_tx
        .try_send(SessionCommand::Prompt(input))
        .map_err(session_command_send_error)
}
```

- [ ] **Step 2: Convert attachments to ACP content blocks**

Add helper:

```rust
fn prompt_input_to_content_blocks(input: &PromptInput) -> Vec<ContentBlock> {
    let mut blocks = Vec::with_capacity(1 + input.attachments.len());
    blocks.push(ContentBlock::Text(TextContent::new(input.text.clone())));

    for attachment in &input.attachments {
        let uri = format!("file://local-attachment/{}", attachment.name);
        let resource = match &attachment.content {
            PromptAttachmentContent::Text(text) => EmbeddedResourceResource::TextResourceContents(
                TextResourceContents::new(uri).text(text.clone()).mime_type(attachment.mime_type.clone()),
            ),
            PromptAttachmentContent::Blob(base64) => EmbeddedResourceResource::BlobResourceContents(
                BlobResourceContents::new(uri, base64.clone()).mime_type(attachment.mime_type.clone()),
            ),
        };
        blocks.push(ContentBlock::Resource(EmbeddedResource::new(resource)));
    }

    blocks
}
```

If builder method names differ in the local crate, use the concrete fields from `agent-client-protocol-schema` and keep the serialized JSON shape exactly:

```json
{ "type": "resource", "resource": { "uri": "file://local-attachment/notes.txt", "text": "hello", "mimeType": "text/plain" } }
```

- [ ] **Step 3: Send `PromptRequest` directly**

In the runtime command loop, replace:

```rust
session.send_prompt(prompt)
```

with:

```rust
let blocks = prompt_input_to_content_blocks(&prompt);
session
    .connection()
    .send_request_to(
        Agent,
        PromptRequest::new(session.session_id().to_string(), blocks),
    )
    .on_receiving_result({
        let update_tx = session.update_tx();
        async move |result| {
            let response = result?;
            update_tx
                .unbounded_send(SessionMessage::StopReason(response.stop_reason))
                .map_err(agent_client_protocol::util::internal_error)?;
            Ok(())
        }
    });
```

If `update_tx` is private, add a small method in the local runtime wrapper or mirror the current `agent-client-protocol` session implementation in a local helper. The checkpoint is serialized provider JSON, not a specific private API.

- [ ] **Step 4: Wire registry to runtime prompt input**

In `crates/lab/src/acp/registry.rs`, add:

```rust
pub async fn prompt_session_with_attachments(
    &self,
    session_id: &str,
    prompt: &str,
    attachments: Vec<crate::dispatch::acp::params::LocalPromptAttachment>,
    principal: &str,
) -> Result<(), ToolError> {
    let runtime_attachments = attachments.into_iter().map(|attachment| {
        let content = match attachment.content {
            crate::dispatch::acp::params::LocalAttachmentContent::Text { text } => {
                crate::acp::runtime::PromptAttachmentContent::Text(text)
            }
            crate::dispatch::acp::params::LocalAttachmentContent::Blob { base64 } => {
                crate::acp::runtime::PromptAttachmentContent::Blob(base64)
            }
        };
        crate::acp::runtime::PromptAttachment {
            id: attachment.id,
            name: attachment.name,
            mime_type: attachment.mime_type,
            size: attachment.size,
            content,
        }
    }).collect();

    self.prompt_session_input(
        session_id,
        crate::acp::runtime::PromptInput {
            text: prompt.to_string(),
            attachments: runtime_attachments,
        },
        principal,
    ).await
}
```

Refactor existing `prompt_session` so ownership checks, state transitions, title fallback, activity touch, and persistence live in one shared internal method.

- [ ] **Step 5: Run Rust ACP contract tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features -E 'test(acp_prompt_)'
```

Expected: PASS. Provider capture shows first block is text and second block is an embedded resource.

## Task 6: Full Verification And Checkpoints

**Files:**
- All files from Tasks 1-5.

- [ ] **Step 1: Run frontend unit tests**

Run:

```bash
cd apps/gateway-admin
pnpm test
```

Expected: PASS for the gateway-admin unit suite.

- [ ] **Step 2: Run browser chat tests**

Run:

```bash
cd apps/gateway-admin
pnpm run test:browser -- lib/browser/chat-shell.browser.test.ts
```

Expected: PASS. Local file picker test confirms attach, remove, and payload.

- [ ] **Step 3: Run focused Rust ACP tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features -E 'test(acp_)'
```

Expected: PASS for ACP route, registry, and contract tests.

- [ ] **Step 4: Run normal lab crate verification**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features
```

Expected: PASS. If unrelated failures exist, record them with test names and rerun the focused passing commands above.

- [ ] **Step 5: Build the frontend export**

Run:

```bash
cd apps/gateway-admin
pnpm build
```

Expected: PASS and `apps/gateway-admin/out/chat/index.html` exists.

- [ ] **Step 6: Final manual checkpoint**

Run a local served UI if needed:

```bash
just run -- serve
```

Expected:

- `/chat/` loads.
- Attach button opens the device picker on desktop.
- Mobile viewport still shows the attach control and send button without overlap.
- Removing an attachment clears the chip before send.
- Oversized and unsupported files show an inline error and do not issue `/v1/acp/.../prompt`.

## Open Questions

- Are the first-slice limits correct: 5 files and 2 MiB per file?
- Should PDF be allowed in the first slice, or should it wait until provider support is verified in a real ACP session?
- Should sent user-message bubbles preserve attachment metadata immediately, or is pre-send preview plus backend handoff enough for this bead?
- Should workspace-file attachments continue to use `kind: "file"` unchanged, or should they be renamed to `kind: "workspace"` in a follow-up migration?

## Out Of Scope

- Drag and drop.
- Directory upload.
- Server-side persistent attachment library.
- Antivirus or content scanning.
- Resumable uploads.
- Google Drive or remote storage providers.
- Enabling provider filesystem read/write capabilities.
