---
title: "Discover alignment"
created: "2026-09-08"
updated: "2026-09-08"
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

Current backing gaps: inspected Labby/Depot code has no canonical popularity,
co-install recommendation, or verified-publisher feed contract. Existing remote
fork and local Skill acquisition are distinct operations. These gaps remain open;
the card/modal refactor does not resolve them.

## Current acceptance disposition

| Requirement | Current implementation | Remaining proof or work |
| --- | --- | --- |
| Hero, catalog summary, integrated search | Implemented; unknown crawl and verification values say Not reported | Real crawl and publisher-verification contracts |
| Source/kind filter popover | Implemented, including narrow viewports and outside-click dismissal | Final deployed visual comparison |
| Three recommendation rows | Not implemented | Canonical popularity, team recency, and loadout-pairing contracts |
| Six discovery feeds | Not implemented; current kind filters are not feed substitutes | Server-ranked, paginated feed semantics |
| Sort, density, layout controls | Implemented for the retained result window | Global feed sorting must remain a separate server contract |
| Artifact cards | Kind, source, identity, description, real revision dates, and publication metadata | Capability chips, canonical metrics, and authorized acquisition actions |
| Centered inspection modal | Implemented with source/revision metadata and explicit focus restoration | Content preview, installation formats, and authority-backed library/fork/activation flows |
| Responsive interactions | Focused browser regression covers desktop and mobile bounds, source selection, modal closure, and density/layout | Full light/dark comparison and real operation persistence |
| Deployment | No Discover changes deployed from this worktree | Integrate onto the qualified native/auth base, then verify production |

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
