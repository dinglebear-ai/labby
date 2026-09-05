export const BROAD_ORIGINS = ["http://*/*", "https://*/*"];

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
