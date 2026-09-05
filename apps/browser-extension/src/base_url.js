const LOOPBACK_HOSTS = new Set(["localhost", "127.0.0.1", "[::1]"]);

/**
 * Parse and normalize the local Labby endpoint. The extension is a privileged
 * browser bridge, so it must never be pointed at a remote control plane.
 *
 * @param {unknown} value
 * @returns {string}
 */
export function parseLoopbackBaseUrl(value) {
  if (typeof value !== "string") throw new Error("invalid_base_url");
  const authority = value.match(/^https?:\/\/(\[[^\]]+\]|[^/:?#]+)(?::\d+)?\/?$/i);
  const rawHost = authority?.[1];
  if (!rawHost || !LOOPBACK_HOSTS.has(rawHost.toLowerCase())) throw new Error("invalid_base_url");
  let url;
  try { url = new URL(value); } catch { throw new Error("invalid_base_url"); }
  if (!['http:', 'https:'].includes(url.protocol) ||
      !LOOPBACK_HOSTS.has(url.hostname.toLowerCase()) ||
      url.username || url.password || url.hash || url.search ||
      (url.pathname !== "/" && url.pathname !== "")) {
    throw new Error("invalid_base_url");
  }
  return url.origin;
}
