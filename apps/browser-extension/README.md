# Labby Browser Bridge extension

Load this directory as an unpacked Manifest V3 extension during development. It connects directly to Labby's loopback `/browser/socket` endpoint and does not require a separate web application or bridge process.

The default `granted_sites` mode scans only tabs whose origin Chrome already permits. Invoking Labby on the current tab grants a one-tab `activeTab` scan from that explicit user gesture without broadening background access. Selecting `all_tabs` triggers Chrome's optional-host-permission prompt and shows a persistent warning. Returning to granted-sites mode removes the broad permission.

Discovery feature-detects `document.modelContext.getTools()` in the page main world. Unsupported pages are ignored; the extension does not emulate WebMCP with DOM automation.

## Credential storage and recovery

Pairing creates a non-extractable Ed25519 private `CryptoKey`. The key is preserved by IndexedDB structured cloning and is never exported to `chrome.storage.local`; that storage contains only non-secret settings and server pairing/association identifiers. Copying ordinary extension settings therefore does not produce an exportable signing credential, but this is not an OS hardware-keystore guarantee: anyone controlling the browser profile and browser process may still be able to use the key through WebCrypto.

There is no key export or recovery phrase. A missing, corrupt, revoked, or legacy JWK identity fails closed: the extension removes the stale association, generates a new non-extractable key, and requires operator-approved pairing again. Removing/reinstalling the extension or clearing its site data has the same recovery consequence. After suspected profile compromise or workstation decommissioning, revoke the paired browser in Labby, clear/reinstall the extension, and approve only the newly displayed pairing request.

```bash
npm ci
npm test
npm run typecheck
```
