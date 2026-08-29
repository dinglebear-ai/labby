# Labby Browser Bridge extension

Load this directory as an unpacked Manifest V3 extension during development. It connects directly to Labby's loopback `/browser/socket` endpoint and does not require a separate web application or bridge process.

The default `granted_sites` mode scans only tabs whose origin Chrome already permits. Invoking Labby on the current tab grants a one-tab `activeTab` scan from that explicit user gesture without broadening background access. Selecting `all_tabs` triggers Chrome's optional-host-permission prompt and shows a persistent warning. Returning to granted-sites mode removes the broad permission.

Discovery feature-detects `document.modelContext.getTools()` in the page main world. Unsupported pages are ignored; the extension does not emulate WebMCP with DOM automation.

```bash
npm ci
npm test
npm run typecheck
```
