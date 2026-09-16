---
title: "Discover alignment"
created: "2026-09-08"
updated: "2026-09-15"
---

# Discover alignment

Reference: [Gateway Console design](https://claude.ai/design/p/d80fe050-1bc9-44b0-aa68-6e873344c619?file=Gateway+Console.dc.html&via=share).

This work aligns the real Discover route with the supplied design. The reference's
sample artifacts, counts, publisher identities, verification badges, and rankings
are illustrative; they must not become production data or authorization policy.

## Required page structure

- Discover hero with publisher action, four-part catalog summary, and integrated search.
- Source shortcuts and a kind/source filter popover that remains usable on mobile.
- Three horizontal recommendation rows: Popular This Week, New From Your Team,
  and Pairs With Your Loadouts.
- Trending, New, Popular, Bundled, Hot Forks, and Curated feeds, with sort, density,
  and layout controls.
- Three-column desktop artifact cards: kind/family icon, source, title, publisher,
  age, description, capability chips, metrics, and action footer.
- Centered artifact modal, with source/revision details, content preview, install
  formats, and supported library/fork/activation actions.

## Interaction and authority

Search and source changes preserve the bounded federation cursor and stale-response
guards. Artifact URLs retain both provider and artifact identity. Closing details
must preserve the current search and source. Source IDs are not interchangeable
with artifact-authority connection IDs.

Operation permissions and supported artifact kinds come from the backend. Do not
promote read-only users or infer publisher verification from provenance. Skill
import/activation must not be presented as a generic deploy operation for all kinds.

Dates use the actual Depot contract: artifact `createdAt`/`updatedAt`, revision
`authoredAt`. Missing dates stay missing; browser-window sorting is not a global
feed ranking. Metadata extensions must remain explicitly allowlisted and bounded
through Depot's public projection, Labby's federation, and the typed UI parser.

## States and responsive behavior

Preserve loading, empty, partial-provider, expired-window, error, and unavailable
capability states. Uncollected metrics are not zero. Unavailable feeds need a clear
explanation rather than fabricated rows. Search stays full-width at narrow sizes;
secondary filtering moves into a popover. Cards collapse to one column. The modal
fits the viewport, scrolls its content, traps focus, and closes on Escape or outside
interaction. Light mode, keyboard focus, and reduced motion use Aurora primitives.

## Completion evidence still required

Implementation and testing of a subset do not complete the goal. Final acceptance
requires comparing the actual route against every structural item above, testing
all represented operations with real authority, checking desktop/mobile and
dark/light states, and verifying the qualified deployed revision and persistence.

Current backing gaps: inspected Labby/Depot code still has no canonical popularity,
co-install recommendation, or verified-publisher feed contract. Existing remote
fork, exact Artifact import, and local Skill acquisition/activation remain distinct
operations. The aligned UI therefore keeps recommendation/feed structure visible
without fabricating server rankings, and exposes only operations whose exact
semantics are backed by the active mode and authority.

## Current acceptance disposition

| Requirement | Current implementation | Remaining proof or work |
| --- | --- | --- |
| Hero, catalog summary, integrated search | Aligned to the reference at desktop; search remains full-width on narrow viewports and mock timestamps are deterministic | Real crawl and publisher-verification contracts remain optional/Not reported when absent |
| Source/kind filter popover | Aligned desktop geometry; mobile hides crowded source shortcuts while retaining every source in the filter panel | Production persistence still depends on the authenticated gateway session |
| Three recommendation rows | Reference-aligned in mock mode; live mode preserves all three rows with an explicit unavailable-evidence explanation | Canonical popularity, team-recency, and loadout-pairing contracts |
| Six discovery feeds | All six reference tabs implemented; mock mode uses illustrative lenses, live mode retains the catalog window and explicitly says canonical feed evidence is unavailable | Server-ranked, paginated feed semantics |
| Sort, density, layout controls | Reference-aligned trigger/menu; grid/list and comfortable/compact geometry verified in-browser | Global feed sorting remains a separate server contract |
| Artifact cards | Reference-aligned grid/list/compact cards, search-match cues, exact mock metrics/spec chips, and truthful live evidence/action suppression | Canonical live recommendation metrics and any additional generic acquisition semantics |
| Centered inspection modal | Mock preview matches the reference desktop/mobile geometry, README/install/upstream content, expandable contents, format menu, and action strip; live mode retains the real provenance/revision/readme/import inspector | Generic live Fork/Send semantics remain intentionally unexposed until authority contracts are exact |
| States | Loading, empty, partial-provider, expired-window, all-failed coverage, and unavailable-feed behavior have explicit render/model tests | End-to-end failed-Depot UI requires an authenticated live gateway session |
| Responsive interactions | Desktop/mobile, grid/list/compact, search/filters, Escape/outside close, focus restoration, no horizontal overflow, and dark/light themes verified in-browser | Re-verify after production deployment |
| Deployment | Branch implementation is committed but not production-verified by this document | Push/integrate onto the qualified native/auth base, deploy, then verify the same state matrix |

### Verification notes, 2026-09-08

The production static export succeeds on the declared Node 22 runtime, including
route bundle budgets and static navigation build-ID checks. This build was run
after mock-mode browser QA so the generated export is not the mock-data export.

The full Node 22 unit run reports 581 passing and 2 failing tests (583 total).
Both failures reproduce in isolation in the unchanged gateway OAuth dialog tests:
`renaming a gateway invalidates an in-flight OAuth start` and
`switching away from OAuth invalidates an in-flight OAuth start`. The same isolated
tests pass on Node 24. That comparison is diagnostic evidence, not a replacement
for the package's declared Node 22 gate; the Node 22 full-suite gate remains open.

A follow-up contract regression reproduces the rejection of Depot's existing
`license.declared` detail field. The parser now accepts a nullable string bounded
to Depot's 1,024-character limit, while still rejecting structured declarations,
oversized values, and unknown license fields. The inspector displays the supplied
declaration without implying that it has been reviewed. The focused 25-test
client/card/model suite and desktop/mobile browser regression pass on Node 22.

### Verification notes, 2026-09-15

A side-by-side Playwright comparison against the locally extracted Gateway Console
reference verified the default 1440 px Discover geometry, search state, empty state,
filter panel, three recommendation rails, six shelf tabs, display-options popover,
grid/list/compact result geometry, and the artifact inspector. The first default
reference/live grid card measures 324.5 x 232 px at the same coordinates; the
reference/live desktop inspector measures 722 x 466 px. At 390 x 844, the aligned
preview inspector measures 342 x 637.140625 px with 24 px side gutters, while the
Discover page remains one column with no horizontal overflow.

Mock-only presentation logic is isolated from production semantics: reference
metrics preserve their illustrative strings, kind teaching chips are derived only
from the mock dataset, the design-time clock is fixed to the mock epoch, and the
illustrative depot add command appears only in preview mode. Production cards and
the production inspector continue to render only returned evidence and exact
Artifact import/provenance operations.

Dark and light modes were exercised through next-themes' persisted preference. The
same card/modal geometry was retained while semantic foreground, surface, border,
and shadow tokens changed as expected. Escape, explicit close, and actual backdrop
interaction close the URL-controlled inspector, and focus returns to the originating
artifact card. Selecting an install-format target closes its popover.

Focused Discover regressions, the full Gateway Admin test suite, full ESLint,
TypeScript, the 63-route production static build, route bundle budgets, and static
build-ID verification passed on the declared Node 22 runtime after these changes.
The repository's broad gateway-detail browser harness was also attempted separately;
it exceeded the execution window and left a Node 24 test-runner child despite an
explicit Node 22 parent, so that harness is not counted as passing evidence here.
The Discover-specific Playwright state matrix described above completed directly.

A disposable non-mock standalone preview was also started from an isolated app copy.
It reached Labby's authentication boundary and rendered the authentication-error
surface before Depot discovery could execute; therefore an end-to-end failed-Depot
visual remains a deployment/authenticated-gateway verification item rather than
being inferred from an unauthenticated dev preview.
