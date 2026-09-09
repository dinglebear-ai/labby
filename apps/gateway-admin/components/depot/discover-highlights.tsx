import { DiscoverFeaturedRail, type FeaturedState } from './discover-featured-rail'

export type DiscoverHighlightsData = {
  popular: FeaturedState
  team: FeaturedState
  loadouts: FeaturedState
}

/** Explicit unsupported states until the corresponding authorized feeds exist. */
const unavailableFeeds: DiscoverHighlightsData = {
  popular: { state: 'unavailable', message: 'Seven-day install activity is not available from the connected catalog.' },
  team: { state: 'unavailable', message: 'Team-authored activity is not available from the connected catalog.' },
  loadouts: { state: 'unavailable', message: 'Co-install recommendations are not available from the connected catalog.' },
}

export function DiscoverHighlights({ feeds = unavailableFeeds, artifactHref }: {
  feeds?: DiscoverHighlightsData
  artifactHref: (providerId: string, artifactId: string) => string
}) {
  return <div aria-label="Featured discovery feeds" className="min-w-0 space-y-3">
    <DiscoverFeaturedRail title="Popular This Week" subtitle="installs, last 7 days" feed={feeds.popular} artifactHref={artifactHref} />
    <DiscoverFeaturedRail title="New From Your Team" subtitle="team activity" feed={feeds.team} artifactHref={artifactHref} />
    <DiscoverFeaturedRail title="Pairs With Your Loadouts" subtitle="co-installed with what you run" feed={feeds.loadouts} artifactHref={artifactHref} />
  </div>
}
