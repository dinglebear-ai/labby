/**
 * @param {unknown} value
 * @returns {boolean}
 */
export function eligibleUrl(value) {
  try {
    const url = new URL(/** @type {string} */ (value));
    return url.protocol === "http:" || url.protocol === "https:";
  } catch {
    return false;
  }
}

/**
 * @param {string} urlValue
 * @param {unknown} [title]
 * @returns {{url: string, title: string}}
 */
export function sanitizePage(urlValue, title = "") {
  const url = new URL(urlValue);
  if (!eligibleUrl(url.href)) throw new Error("ineligible_url");
  return {
    url: `${url.origin}${url.pathname || "/"}`,
    title: String(title).replace(/[\u0000-\u001f\u007f]/g, "").slice(0, 200)
  };
}

/**
 * @param {chrome.tabs.Tab | undefined} tab
 * @param {typeof chrome.permissions} permissionsApi
 * @returns {Promise<boolean>}
 */
export async function canScanTab(tab, permissionsApi) {
  if (!tab?.id || tab.incognito || !eligibleUrl(tab.url)) return false;
  const origin = new URL(/** @type {string} */ (tab.url)).origin + "/*";
  return permissionsApi.contains({origins: [origin]});
}

/**
 * @param {unknown} tools
 * @returns {Array<{name: string, title: string, description: string, input_schema: unknown, origin: string, annotations: {read_only_hint: boolean, untrusted_content_hint: boolean}}>}
 */
export function normalizeTools(tools) {
  if (!Array.isArray(tools) || tools.length === 0 || tools.length > 64) return [];
  return tools.flatMap((tool) => {
    if (!tool || typeof tool.name !== "string" || tool.name.length < 1 || tool.name.length > 128) return [];
    let inputSchema = tool.inputSchema ?? tool.input_schema ?? {};
    if (typeof inputSchema === "string") {
      try { inputSchema = JSON.parse(inputSchema); } catch { return []; }
    }
    const annotations = tool.annotations ?? {};
    return [{
      name: tool.name,
      title: typeof tool.title === "string" ? tool.title.slice(0, 200) : "",
      description: typeof tool.description === "string" ? tool.description.slice(0, 1000) : "",
      input_schema: inputSchema,
      origin: typeof tool.origin === "string" ? tool.origin.slice(0, 256) : "",
      annotations: {
        read_only_hint: (annotations.readOnlyHint ?? annotations.read_only_hint) === true,
        untrusted_content_hint: (annotations.untrustedContentHint ?? annotations.untrusted_content_hint) === true
      }
    }];
  }).sort((left, right) => left.name.localeCompare(right.name));
}

/**
 * @param {chrome.tabs.Tab} tab
 * @param {{result?: {supported?: boolean, tools?: unknown}, documentId?: string} | undefined} injectionResult
 */
export function buildObservation(tab, injectionResult) {
  const tools = normalizeTools(injectionResult?.result?.tools);
  // A tab with no id cannot be addressed for a later invocation, so it is not
  // a usable observation even if the page did expose tools.
  if (tab?.id === undefined) return null;
  if (!injectionResult?.result?.supported || tools.length === 0 || !injectionResult?.documentId) return null;
  return {
    ...sanitizePage(/** @type {string} */ (tab.url), tab.title),
    tools,
    tab_id: tab.id,
    document_id: injectionResult.documentId
  };
}

/**
 * Canonical JSON for a catalog: object keys are sorted so that a comparison is
 * about content, not key order. Shared with the injected `invokeWebMcp`, which
 * must reproduce it inline -- see src/probe.js.
 *
 * @param {unknown} value
 * @returns {string}
 */
export function stableStringify(value) {
  /** @type {(item: unknown) => unknown} */
  const stable = (item) => Array.isArray(item) ? item.map(stable) :
    item && typeof item === "object" ? Object.fromEntries(Object.keys(item).sort().map((key) => [key, stable(/** @type {Record<string, unknown>} */ (item)[key])])) : item;
  return JSON.stringify(stable(value));
}

/**
 * Return the tab ids whose active observations belong to a newly ignored
 * origin. Keeping this selection pure makes the consent-reconciliation rule
 * independently testable from the service worker's Chrome event wiring.
 *
 * @param {Iterable<{tab_id: number, url: string}>} observations
 * @param {unknown} ignoredOrigins
 * @returns {number[]}
 */
export function ignoredObservationTabIds(observations, ignoredOrigins) {
  if (!Array.isArray(ignoredOrigins)) return [];
  const ignored = new Set(ignoredOrigins.filter((origin) => typeof origin === "string"));
  return [...observations].flatMap((observation) => {
    try { return ignored.has(new URL(observation.url).origin) ? [observation.tab_id] : []; }
    catch { return []; }
  });
}
