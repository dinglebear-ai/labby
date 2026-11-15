export function loadedLevelsMatch(levels, expectedLevel) {
  return levels.length > 0 && levels.every(level => level === expectedLevel)
}
