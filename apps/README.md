# Apps

Frontend applications that live in-repo but are not part of the Rust workspace yet.

- `web/`: Next.js operator UI embedded by the gateway (`gateway-admin` package)
- `chrome/`: browser extension (`@dinglebear-ai/labby-browser-extension` package)
- `tauri/`: native desktop shell (`labby-desktop` package)

- `liveview/`: Phoenix LiveView console integration target (`labby_console`), a separate BEAM runtime. See [integration plan](liveview/README.md).
