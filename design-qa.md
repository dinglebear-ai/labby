# Labby for Unraid fidelity QA

**Source visual truth**

- `Labby for Unraid.html`
- `outputs/labby-fidelity-pass/final_runs/run_3/screenshots/final_execution_1_reference_gateway.png`
- Source pixels: 1280 × 1800 RGBA.

**Rendered implementation**

- Installed native Unraid plugin route: `Settings/Labby`
- Plugin package: `1.3.11`
- `outputs/labby-fidelity-pass/final_runs/run_3/screenshots/final_execution_2_live_overview.png`
- `outputs/labby-fidelity-pass/final_runs/run_3/screenshots/final_execution_3_live_gateway.png`
- `outputs/labby-fidelity-pass/final_runs/run_3/screenshots/final_execution_4_add_server.png`
- `outputs/labby-fidelity-pass/final_runs/run_3/screenshots/final_execution_5_incus_settings.png`
- Implementation pixels: 1280 × 1800 RGBA; CSS viewport 1280 × 1800; device scale factor 1.
- State: authenticated Unraid 7.3.2, native Labby runtime, three connected HTTP upstreams, Incus settings temporarily expanded without saving.

**Full-view comparison evidence**

- `outputs/labby-fidelity-pass/final_runs/run_3/screenshots/final_execution_6_reference_and_live_gateway.png`
- Source and implementation are combined in one 2560 × 1800 comparison image with no scaling.
- Native Unraid chrome above the Labby shell is required host UI and is excluded from inner-shell fidelity findings.

**Focused comparison evidence**

- The full-width normalized comparison keeps all important gateway details readable, so a separate crop is not required.
- Measured source-aligned implementation geometry:
  - summary card: x 20, width 1240, height 201;
  - server-list card: x 20, width 1240, relative y 305 beneath the 64 px Labby header;
  - filter row: 52 px;
  - live server rows: 68 px, versus approximately 58 px passive rows in the mock. The extra 10 px retains live freshness and three working action controls.

**Required fidelity surfaces**

- Fonts and typography: native Unraid Clear Sans/system rendering is retained. Heading scale, eyebrow tracking, title case, monospace endpoint/version treatment, weights, and wrapping match the source hierarchy.
- Spacing and layout rhythm: 20 px content inset, 18 px inter-card gap, 1240 px cards, 201 px summary card, split summary/list hierarchy, filter width, borders, six-pixel radii, and shadow density match the source.
- Colors and visual tokens: `#f2f2f2` canvas, white cards, `#1c1b1b` ink, neutral grays, red/orange identity gradient, green/red state pills, and blue HTTP transport pills match the mock.
- Image quality and asset fidelity: the source has no raster product imagery. The implementation uses Unraid's installed Font Awesome assets for the power, reload, action, detail, and copy icons; no placeholders or approximate custom drawings were introduced.
- Copy and content: product labels follow the source. Version, endpoint, counts, health, server rows, runtime, and capabilities are deliberately live rather than copied sample data.
- Accessibility and behavior: semantic tabs, title-case accessible buttons, focus treatment, live filtering, Add Server, endpoint copy, expandable actions/details, native/Incus settings, and the Dashboard toggle remain operational.

**Intentional upgrades preserved**

- Native Unraid host chrome and plugin lifecycle.
- Live gateway state instead of sample telemetry.
- Settings tab, native/Incus deployment controls, backup-first persistence, and service controls.
- Dashboard widget setting and aggregate health endpoint.
- Status freshness, endpoint copy, enable/disable, retry, cleanup, remove, and detail actions.

**Comparison history**

1. P1: the live gateway summary and server table were one continuous card while the source uses two elevated regions. Fix: split the live summary and list into separate cards. Post-fix evidence: run 3 combined comparison.
2. P1: the inner content began at 12 px and summary/list heights did not match. Fix: measured and set the exact 20 px inset, 201 px summary height, and relative list y 305. Post-fix browser geometry assertions pass.
3. P2: global Unraid styles forced uppercase labels and oversized margins, making rows and actions substantially less dense than the source. Fix: scoped title case, reset margins, compact copy controls, and reduced row padding. Post-fix live rows are 68 px and remain fully functional.
4. P2: the endpoint rendered as `:8765` and omitted `/mcp`. Fix: fall back to `tower` for an empty server name and render `tower:8765/mcp`.
5. P2: HTTP transport lacked the source's blue semantic treatment and rows lacked state rails. Fix: added transport-specific pills and live state rails.

**Findings**

- No actionable P0, P1, or P2 findings remain.
- P3: the source mock includes calls/minute, clients, uptime bars, history charts, and error-rate telemetry that the current gateway surface does not expose. The plugin does not fabricate them.
- P3: live rows are 10 px taller than the mock's passive rows because the production plugin exposes freshness and working management controls.

**Primary interactions tested**

- Switched Overview, Gateway, and Settings tabs.
- Filtered the live gateway to one server and cleared the filter.
- Opened and closed the HTTP/stdio Add Server controls.
- Switched the deployment selector to Incus, verified the Incus controls, and restored native without submitting.
- Checked the installed version and displayed MCP endpoint.
- Checked browser console errors and failed same-origin requests: none.

**Implementation checklist**

- [x] Match source shell geometry and component hierarchy.
- [x] Preserve live data and native Unraid integration.
- [x] Preserve native and Incus operational controls.
- [x] Preserve Dashboard widget functionality.
- [x] Verify package checksums and native/Incus runtime behavior.
- [x] Verify browser interactions and diagnostics.

final result: passed
