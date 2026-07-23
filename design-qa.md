# Labby for Unraid design QA

**Source visual truth**

- `Labby for Unraid.html`
- `outputs/labby-settings-reference/final_runs/run_1/screenshots/reference_overview.png`
- Source pixels: 1280 x 1800 RGBA.

**Rendered implementation**

- Native Unraid plugin route: `http://tower/Settings/Labby`
- `outputs/labby-unraid-plugin/final_runs/run_1/screenshots/final_execution_1_native_overview.png`
- `outputs/labby-unraid-plugin/final_runs/run_1/screenshots/final_execution_2_gateway.png`
- `outputs/labby-unraid-plugin/final_runs/run_1/screenshots/final_execution_2b_gateway_drawer.png`
- `outputs/labby-unraid-plugin/final_runs/run_1/screenshots/final_execution_3_incus_settings.png`
- Implementation pixels: 1280 x 1800 RGBA; CSS viewport 1280 x 1800; device scale factor 1.
- State: authenticated Unraid 7.3.2 Settings > Labby plugin page, live native runtime, live empty gateway catalog, Incus settings expanded for inspection.

**Full-view comparison evidence**

- `outputs/labby-unraid-plugin/final_runs/run_1/screenshots/reference_and_native_gateway.png`
- The implementation is shown inside the real Unraid webGUI chrome; the source is the inner Labby product shell only. That host chrome is required by the selected native plugin surface and is excluded from fidelity findings.
- Dynamic server rows and telemetry in the source are sample data. The implementation correctly renders current live values from the test box (zero configured upstreams) and does not substitute mock data.

**Focused comparison evidence**

- `outputs/labby-unraid-plugin/final_runs/run_1/screenshots/focused_gateway_shell_comparison.png`
- The normalized inner-shell crop verifies the 3 px red-to-orange rail, 60 px product header, power identity, status pill, segmented tabs, endpoint/version treatment, card geometry, heading hierarchy, action placement, live summary grid, empty state, palette, radii, borders, and shadow density.
- Focused Settings inspection verifies that native and Incus fields retain the Unraid definition-list alignment and that the Incus group expands without breaking the form grid.

**Required fidelity surfaces**

- Fonts and typography: clear-sans/system fallbacks, weights, 26 px primary headings, uppercase tracking, monospace endpoint/version text, and compact control labels match the source hierarchy. Native Unraid font rendering is intentionally retained.
- Spacing and layout rhythm: shell width, 20 px page inset, 16 px section gaps, 6 px radii, card borders/shadows, segmented navigation, summary grid, and action alignment match the source. The mandatory Unraid header adds host chrome above the shell but does not alter its internal rhythm.
- Colors and visual tokens: `#f2f2f2`, white cards, `#1c1b1b` ink, neutral grays, red/orange brand gradient, and green/red state colors match the source and Unraid component contract.
- Image quality and asset fidelity: the source contains no raster product imagery. The implementation uses Unraid's installed Font Awesome icon for the power mark; no placeholder imagery, custom SVG, emoji, or generated substitute is present.
- Copy and content: static Labby labels and control language follow the source. Counts, runtime labels, endpoint, status, and server content are deliberately live.
- Accessibility and behavior: semantic tablist/tab roles and `aria-selected` states are present; keyboard-native buttons, form inputs, visible focus treatment, labels, and responsive rules are retained.

**Primary interactions tested**

- Switched Overview, Gateway, and Settings tabs.
- Exercised the server filter.
- Opened and closed the Add Server controls.
- Switched the deployment selector to Incus, verified all Incus controls, and restored the original native selection without submitting.
- Checked browser console errors and failed same-origin requests: none.

**Comparison history**

1. P1: the Incus field group initially escaped the definition-list layout when expanded. Fix: enabled Markdown parsing on the conditional group. Post-fix evidence: `final_execution_3_incus_settings.png`.
2. P1: browser diagnostics found invalid HTML `pattern` expressions under Firefox's Unicode Sets validation. Fix: replaced ambiguous hyphen/slash class members with explicit hexadecimal escapes. Post-fix evidence: clean `final_script_log.txt` diagnostics.
3. P2: the Gateway state initially omitted the source's summary band. Fix: added a live eight-column gateway summary using actual upstream/runtime counts. Post-fix evidence: `focused_gateway_shell_comparison.png`.

**Findings**

- No actionable P0, P1, or P2 findings remain.
- P3: the source mock shows call-rate, client-count, uptime-history, and sample server telemetry that the current gateway CLI does not expose. The plugin shows only values it can obtain from the live runtime rather than fabricating those metrics.

**Implementation checklist**

- [x] Match the selected shell and component language.
- [x] Keep the implementation inside the native Unraid plugin route.
- [x] Wire all visible controls to real plugin/gateway operations.
- [x] Preserve backup-first Incus/native deployment persistence.
- [x] Verify live authenticated rendering and interactions.
- [x] Verify browser console and request cleanliness.

final result: passed
