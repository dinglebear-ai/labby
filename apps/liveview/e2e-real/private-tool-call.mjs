export function privateToolCall(tools, upstream, localName, args) {
  const qualified = `${upstream}::${localName}`
  // Ordinary serving exposes raw names. Aggregate serving qualifies its catalog.
  // Both admit the same explicit selector, whose runtime lookup checks ownership.
  if (tools.some(tool => tool.name === localName || tool.name === qualified)) {
    return {name: qualified, arguments: args}
  }
}
