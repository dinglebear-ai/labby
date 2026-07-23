# Design QA

## Comparison Target

- Source visual truth: `/home/jmagar/workspace/labby/outputs/labby-settings-reference/final_runs/run_1/screenshots/reference_gateway.png`
- Implementation: `/home/jmagar/workspace/labby/outputs/labby-settings-reference/final_runs/run_3/screenshots/final_execution_1_core_settings.png`
- Combined comparison: `/home/jmagar/workspace/labby/outputs/labby-settings-reference/final_runs/run_3/screenshots/reference_and_settings_comparison.png`
- Interactive Deployment state: `/home/jmagar/workspace/labby/outputs/labby-settings-reference/final_runs/run_3/screenshots/final_execution_3_saved_preference.png`
- Viewport and CSS size: 1280 x 1800
- Source pixels: 1280 x 1800
- Implementation pixels: 1280 x 1800
- Device scale factor: 1
- Density normalization: none required
- Theme: light
- State: Gateway source screen compared with the new Core Settings screen. The source mock contains Overview and Gateway states but no literal Settings state, so the comparison evaluates the shared shell, header, cards, controls, density, and visual language. The settings form structure is grounded in Unraid's `SettingsGrid.vue` and `CardWrapper.vue` contracts.

## Full-view Comparison Evidence

The combined image shows the source and implementation at the same viewport and density. The implementation preserves the 3px red-to-orange top rail, white 60px header, black/orange power mark, compact segmented primary navigation, 1440px content frame, pale gray page background, white 6px-radius cards, light borders and shadows, dense type scale, muted monospace metadata, and orange/red primary action treatment.

## Focused Region Evidence

The header and first content card are legible at the comparison size, so a separate crop was not needed. The Core implementation screenshot clearly exposes the navigation, card header, 35/65 label-control grid, source/risk/apply badges, text input, and footer actions. The Deployment screenshots separately verify the switch and saved state.

## Required Fidelity Surfaces

- Fonts and typography: system sans and monospace fallbacks reproduce the source hierarchy, compact weights, uppercase eyebrow tracking, dense metadata, and control sizing without wrapping or truncation.
- Spacing and layout rhythm: header height, outer 20px inset, card gaps, 6px radii, dividers, shadows, row density, and action alignment match the source grammar.
- Colors and visual tokens: the implementation uses the source's `#f2f2f2` page, white surfaces, `#1c1b1b` primary text, muted grays, green configured state, and `#e22828` to `#ff8c2f` identity gradient.
- Image quality and asset fidelity: no raster imagery is required. The visible power mark uses the existing Lucide icon library in the source-sized slot; there are no placeholder, generated, or low-resolution assets.
- Copy and content: settings-specific copy describes the actual Labby gateway behavior, backup-first writes, stale-value protection, and Incus provisioning preference. It does not copy Gateway-only mock data into an unrelated screen.

## Findings

No actionable P0, P1, or P2 differences remain.

- P3: the local Next.js development indicator appears at the bottom-left of the browser capture. It is development-only browser chrome and is absent from the static production build.

## Open Questions

None. The mock did not define a Settings state, so the new page deliberately extends its shared visual system while using the upstream Unraid settings component contracts for form layout and behavior.

## Comparison History

1. Initial implementation used the existing cyan Labby SVG and left the standard application sidebar mounted behind the fixed settings shell.
2. The logo was replaced with the source-sized black/orange Lucide power mark, and the standard sidebar now returns no markup on settings routes.
3. Post-fix evidence: `reference_and_settings_comparison.png` confirms the corrected header identity and shell composition. Browser output confirms no duplicate sidebar content, successful Deployment save, and zero console errors.

## Primary Interactions Tested

- Opened `/settings/core/`.
- Navigated to `/settings/deployment/`.
- Toggled `Install android-sdk on provision`.
- Checked the backup-first confirmation.
- Saved and observed the clean `No unsaved changes` state.
- Checked browser console errors: zero.

## Implementation Checklist

- [x] Match the mock shell and visual tokens.
- [x] Adapt the Unraid settings grid and card contracts.
- [x] Wire Core and Deployment routes to the setup settings API.
- [x] Verify the primary settings write flow in Firefox.
- [x] Compare source and implementation in one normalized image.

final result: passed
