const LOOPBACK_HOSTS = new Set(["localhost", "127.0.0.1", "[::1]"]);

/**
 * Parse and normalize a Labby endpoint. Plain HTTP is allowed only for the
 * loopback development endpoint; remote Labby services must use HTTPS.
 *
 * @param {unknown} value
 * @returns {string}
 */
export function parseBaseUrl(value) {
  if (typeof value !== "string") throw new Error("invalid_base_url");
  const authority = value.match(/^https?:\/\/(\[[^\]]+\]|[^/:?#]+)(?::\d+)?\/?$/i);
  const rawHost = authority?.[1]?.toLowerCase();
  if (!rawHost) throw new Error("invalid_base_url");
  let url;
  try { url = new URL(value); } catch { throw new Error("invalid_base_url"); }
  if (!["http:", "https:"].includes(url.protocol) ||
      url.username || url.password || url.hash || url.search ||
      (url.pathname !== "/" && url.pathname !== "")) {
    throw new Error("invalid_base_url");
  }
  const loopback = LOOPBACK_HOSTS.has(rawHost) && LOOPBACK_HOSTS.has(url.hostname.toLowerCase());
  if (!loopback && url.protocol !== "https:") throw new Error("invalid_base_url");
  return url.origin;
}
