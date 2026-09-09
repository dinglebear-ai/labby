import ts from 'typescript'

// Read only the literal catalog table. Never evaluate the supplied HTML or JS.
export function readReferenceCatalog(html) {
  if (Buffer.byteLength(html) > 16 * 1024 * 1024) throw new Error('Reference exceeds 16 MiB')
  const encoded = html.match(/<script type="__bundler\/template">\s*([\s\S]*?)<\/script>/)?.[1]
  if (!encoded) throw new Error('Reference template missing')
  const template = JSON.parse(encoded)
  const marker = 'this._depot = ['
  const start = template.indexOf(marker)
  const end = template.indexOf('];', start)
  if (start < 0 || end < 0 || end - start > 65536) throw new Error('Bounded reference catalog missing')
  const source = ts.createSourceFile('reference.js', `const data = ${template.slice(start + marker.length - 1, end + 1)};`, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS)
  if (source.parseDiagnostics.length || source.statements.length !== 1) throw new Error('Invalid reference catalog syntax')
  const declaration = source.statements[0].declarationList?.declarations[0]
  if (!declaration || !ts.isArrayLiteralExpression(declaration.initializer)) throw new Error('Expected catalog array')
  function literal(node, depth = 0) {
    if (depth > 4) throw new Error('Reference nesting exceeds bound')
    if (ts.isStringLiteral(node)) return node.text
    if (ts.isNumericLiteral(node)) return Number(node.text)
    if (node.kind === ts.SyntaxKind.TrueKeyword) return true
    if (node.kind === ts.SyntaxKind.FalseKeyword) return false
    if (ts.isArrayLiteralExpression(node)) return node.elements.map(value => literal(value, depth + 1))
    if (ts.isObjectLiteralExpression(node)) {
      const result = Object.create(null)
      for (const property of node.properties) {
        if (!ts.isPropertyAssignment(property) || !(ts.isIdentifier(property.name) || ts.isStringLiteral(property.name))) throw new Error('Only literal properties are allowed')
        const key = property.name.text
        if (['__proto__', 'prototype', 'constructor'].includes(key)) throw new Error('Unsafe reference property')
        result[key] = literal(property.initializer, depth + 1)
      }
      return result
    }
    throw new Error('Only literal reference data is allowed')
  }
  const entries = declaration.initializer.elements
  if (!entries.length || entries.length > 200) throw new Error('Reference catalog size invalid')
  return entries.map(entry => {
    if (!ts.isCallExpression(entry) || !ts.isIdentifier(entry.expression) || entry.expression.text !== 'A' || entry.arguments.length !== 11) throw new Error('Expected literal catalog row')
    const [name, kind, publisher, source, description, tags, stars, installs, forks, updated, extra] = entry.arguments.map(node => literal(node))
    if (![name, kind, publisher, source, description, stars, installs, updated].every(value => typeof value === 'string') || !Array.isArray(tags) || !tags.every(tag => typeof tag === 'string') || typeof forks !== 'number' || !extra || Array.isArray(extra) || typeof extra !== 'object') throw new Error('Invalid reference row fields')
    return { name, kind, publisher, source, description, tags, stars, installs, forks, updated, ...extra }
  })
}

const compactCount = value => {
  const match = /^(\d+(?:\.\d+)?)(k|m)?$/i.exec(value)
  if (!match) throw new Error('Invalid reference count')
  return Math.round(Number(match[1]) * ({ k: 1000, m: 1000000 }[match[2]?.toLowerCase()] ?? 1))
}
const ageHours = value => {
  const match = /^(\d+)(h|d|w) ago$/.exec(value)
  if (!match) throw new Error('Invalid reference age')
  return Number(match[1]) * { h: 1, d: 24, w: 168 }[match[2]]
}

export function referenceFixture(html, now = Date.now()) {
  const catalog = readReferenceCatalog(html)
  const rows = catalog.map((item, index) => {
    const id = `reference-${index + 1}`
    const revision = `sha256:${String(index + 1).padStart(64, '0')}`
    const date = new Date(now - ageHours(item.updated) * 3600000).toISOString()
    return {
      providerId: ['tootie.tv', 'jmagar'].includes(item.publisher) ? 'team' : 'catalog', artifactId: id, id,
      kind: item.kind.toLowerCase(), name: item.name, title: item.name, namespace: item.publisher,
      description: item.description, descriptor: { tags: item.tags },
      publisherVerified: item.verified === true,
      ...(['Public', 'Team', 'Private'].includes(item.vis) ? { publication: { visibility: item.vis.toLowerCase() } } : {}),
      metrics: { stars: compactCount(item.stars), installs: compactCount(item.installs), forks: item.forks },
      sourceOrigin: { 'MCP Registry': 'mcp-registry', 'ACP Registry': 'acp-registry', ARD: 'ard', 'skills.sh': 'skills-sh', GitHub: 'github', Claude: 'claude', Gemini: 'gemini', 'Agent Plugins': 'agent-plugins', 'Web Crawl': 'web-crawl' }[item.source] ?? null,
      currentRevisionId: revision, currentRevision: { id: revision, contentDigest: revision, authoredAt: date }, firstSeenAt: date,
    }
  })
  const featured = indices => ({ state: 'ready', items: indices.slice(0, 8).map(index => ({ artifact: rows[index], installs: compactCount(catalog[index].installs) })) })
  const indices = catalog.map((_, index) => index)
  return {
    memberArtifactIds: indices.filter(index => catalog[index].mine === true).map(index => rows[index].artifactId),
    // The supplied prototype's initial grid uses its fixture fork-count order.
    // This is preview ordering only, not a production Trending implementation.
    rows: [...indices].sort((a, b) => catalog[b].forks - catalog[a].forks).map(index => rows[index]),
    highlights: {
      popular: featured([...indices].sort((a, b) => compactCount(catalog[b].installs) - compactCount(catalog[a].installs))),
      team: featured(indices.filter(index => ['tootie.tv', 'jmagar'].includes(catalog[index].publisher)).sort((a, b) => ageHours(catalog[a].updated) - ageHours(catalog[b].updated))),
      loadouts: featured(indices.filter(index => !catalog[index].mine)),
    },
  }
}
