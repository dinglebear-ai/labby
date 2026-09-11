export const BROAD_ORIGINS = ["http://*/*", "https://*/*"];

/** @param {string} baseUrl */
export function labbyOriginPermission(baseUrl) {
  const url = new URL(baseUrl);
  return `${url.protocol}//${url.hostname}/*`;
}

/** @param {typeof chrome.permissions} permissionsApi @param {string} baseUrl */
export async function hasLabbyOriginPermission(permissionsApi, baseUrl) {
  return permissionsApi.contains({origins: [labbyOriginPermission(baseUrl)]});
}

/**
 * Request only the configured Labby host. Chrome match patterns do not carry
 * ports, so one host permission covers the configured HTTP(S)/WebSocket port.
 * @param {typeof chrome.permissions} permissionsApi
 * @param {string} baseUrl
 */
export async function ensureLabbyOriginPermission(permissionsApi, baseUrl) {
  const origins = [labbyOriginPermission(baseUrl)];
  if (await hasLabbyOriginPermission(permissionsApi, baseUrl)) return true;
  return permissionsApi.request({origins});
}

/**
 * @param {typeof chrome.permissions} permissionsApi
 */
export async function enableAllTabs(permissionsApi) {
  return permissionsApi.request({origins: BROAD_ORIGINS});
}

/**
 * @param {typeof chrome.permissions} permissionsApi
 */
export async function disableAllTabs(permissionsApi) {
  const removed = await permissionsApi.remove({origins: BROAD_ORIGINS});
  const stillBroad = await permissionsApi.contains({origins: BROAD_ORIGINS});
  return {removed, stillBroad};
}

/**
 * @param {typeof chrome.permissions} permissionsApi
 * @param {chrome.storage.StorageArea} storageApi
 * @returns {Promise<string>}
 */
export async function reconcileModeAfterRemoval(permissionsApi, storageApi) {
  const broad = await permissionsApi.contains({origins: BROAD_ORIGINS});
  const {scanningMode} = /** @type {{scanningMode?: string}} */ (await storageApi.get("scanningMode"));
  if (!broad && scanningMode === "all_tabs") {
    await storageApi.set({scanningMode: "granted_sites"});
    return "granted_sites";
  }
  return scanningMode || "granted_sites";
}
