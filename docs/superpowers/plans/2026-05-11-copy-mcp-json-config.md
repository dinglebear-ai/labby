# Copy MCP JSON Config Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a "copy to clipboard" feature for the MCP JSON config of a server in the `gateway-admin` web UI.

**Architecture:** Add a Copy button to the "Client Configuration" section in the `GatewayDetailContent` component. The button will use `navigator.clipboard.writeText` and provide visual feedback using a checkmark icon.

**Tech Stack:** React, Next.js, Tailwind CSS, Lucide React, Sonner (for toasts).

---

### Task 1: Add Copy to Clipboard functionality to GatewayDetailContent

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-detail-content.tsx`

- [ ] **Step 1: Add `Check` icon to Lucide imports**

Modify the `lucide-react` import at the top of the file to include `Check`.

- [ ] **Step 2: Add `copied` state to `GatewayDetailContent`**

```tsx
  const [configCopied, setConfigCopied] = useState(false)
```

- [ ] **Step 3: Implement `handleCopyConfig` function**

```tsx
  const handleCopyConfig = async () => {
    try {
      await navigator.clipboard.writeText(clientConfigJson)
      setConfigCopied(true)
      toast.success('Configuration copied to clipboard')
      setTimeout(() => setConfigCopied(false), 2000)
    } catch {
      toast.error('Failed to copy configuration to clipboard')
    }
  }
```

- [ ] **Step 4: Update the UI to include the Copy button**

Locate the "config" tab content and add the `Button` to the "Client JSON" header bar.

```tsx
          <TabsContent value="config">
            <div className="rounded-lg border bg-aurora-panel-medium p-5">
              <div className="mb-4">
                <h2 className="text-lg font-semibold">Client Configuration</h2>
                <p className="text-sm text-aurora-text-muted mt-1">
                  Add this JSON block to your MCP client configuration to connect to this server.
                </p>
              </div>
              <div className="overflow-hidden rounded-aurora-2 border bg-aurora-page-bg">
                <div className="flex items-center justify-between border-b px-4 py-3">
                  <p className="text-xs font-medium uppercase tracking-[0.16em] text-aurora-text-muted">Client JSON</p>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="h-7 w-7 text-aurora-text-muted hover:text-aurora-text-primary"
                    onClick={handleCopyConfig}
                    title="Copy to clipboard"
                  >
                    {configCopied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
                  </Button>
                </div>
                <pre className="aurora-scrollbar overflow-x-auto whitespace-pre-wrap break-all px-4 py-4 text-sm leading-6 text-aurora-text-primary">
                  <code>{clientConfigJson}</code>
                </pre>
              </div>
            </div>
          </TabsContent>
```

- [ ] **Step 5: Verify the changes**

Run the app (if possible) or verify the code structure. Since I cannot run the app easily in this environment, I will rely on code review and manual verification of the logic.

- [ ] **Step 6: Commit the changes**

```bash
git add apps/gateway-admin/components/gateway/gateway-detail-content.tsx
git commit -m "feat(webui): add copy button for mcp json config"
```
