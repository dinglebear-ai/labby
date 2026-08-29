/**
 * Window event that opens the app command palette. Lives outside the palette
 * component so the console topbar (and any other trigger) can dispatch it
 * without importing the palette's module graph.
 */
export const OPEN_COMMAND_PALETTE_EVENT = 'labby:open-command-palette'

/** Opens the command palette directly on its real inline Add Server sheet. */
export const OPEN_ADD_SERVER_PALETTE_EVENT = 'labby:open-add-server-palette'
