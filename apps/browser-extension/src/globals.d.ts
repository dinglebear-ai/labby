/**
 * Globals the extension itself installs in a page's main world.
 *
 * These are Labby's own, not part of any specification -- declaring them here
 * keeps the WebMCP surface in `webmcp-types` the only externally-owned
 * contract the probe is checked against.
 */

/**
 * In-flight WebMCP tool calls for this document, keyed by call id, so a
 * cancellation can abort the exact call it names.
 */
declare var __webbyToolCalls: Map<string, AbortController | {
  cancelled: boolean;
  controller: AbortController | null;
}> | undefined;
