/**
 * Window event that opens the app command palette. Lives outside the palette
 * component so the console topbar (and any other trigger) can dispatch it
 * without importing the palette's module graph.
 */
export const OPEN_COMMAND_PALETTE_EVENT = 'labby:open-command-palette'
