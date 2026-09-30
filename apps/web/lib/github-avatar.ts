const GITHUB_HOSTS = new Set(['github.com', 'www.github.com'])

function normalizeGithubRepoUrl(repoUrl: string): string | null {
  const trimmed = repoUrl.trim()
  if (!trimmed) return null

  const sshMatch = /^git@github\.com:([^/\s]+)\/([^/\s]+?)(?:\.git)?$/i.exec(trimmed)
  if (sshMatch?.[1] && sshMatch[2]) {
    return `https://github.com/${sshMatch[1]}/${sshMatch[2]}`
  }

  return trimmed
}

/**
 * Derive a GitHub avatar URL from a repository URL.
 *
 * Returns `https://github.com/<owner>.png` for any GitHub repo URL, which
 * resolves to the owner's avatar whether the owner is a user or an org.
 * Returns `null` for non-GitHub URLs, missing URLs, or unparseable input.
 */
export function githubAvatarFromRepoUrl(repoUrl: string | undefined | null): string | null {
  if (!repoUrl) return null
  let parsed: URL
  try {
    const normalized = normalizeGithubRepoUrl(repoUrl)
    if (!normalized) return null
    parsed = new URL(normalized)
  } catch {
    return null
  }
  if (!GITHUB_HOSTS.has(parsed.hostname.toLowerCase())) return null
  const segments = parsed.pathname.split('/').filter(Boolean)
  const owner = segments[0]
  if (!owner) return null
  return `https://github.com/${encodeURIComponent(owner)}.png?size=120`
}
