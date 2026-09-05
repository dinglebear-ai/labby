const IDENTITY_VERSION = 1;
const DATABASE_NAME = "labby-browser-identity";
const STORE_NAME = "credentials";
const RECORD_KEY = "active";
const LEGACY_KEYS = ["privateKey", "publicKey"];
const ASSOCIATION_KEYS = ["browserId", "pairingId"];

/** @param {IDBRequest<any>} request @returns {Promise<any>} */
function requestResult(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error || new Error("identity_store_failed"));
  });
}

/** IndexedDB preserves a non-extractable CryptoKey through structured cloning. */
export class IndexedDbIdentityStore {
  /** @param {IDBFactory} factory */
  constructor(factory) {
    this.factory = factory;
  }

  async database() {
    const request = this.factory.open(DATABASE_NAME, IDENTITY_VERSION);
    request.onupgradeneeded = () => {
      if (!request.result.objectStoreNames.contains(STORE_NAME)) {
        request.result.createObjectStore(STORE_NAME);
      }
    };
    return /** @type {Promise<IDBDatabase>} */ (requestResult(request));
  }

  async get() {
    const database = await this.database();
    try {
      return await requestResult(database.transaction(STORE_NAME).objectStore(STORE_NAME).get(RECORD_KEY));
    } finally {
      database.close();
    }
  }

  /** @param {(store: IDBObjectStore) => void} update */
  async write(update) {
    const database = await this.database();
    try {
      const transaction = database.transaction(STORE_NAME, "readwrite");
      update(transaction.objectStore(STORE_NAME));
      await new Promise((resolve, reject) => {
        transaction.oncomplete = resolve;
        transaction.onerror = () => reject(transaction.error || new Error("identity_store_failed"));
        transaction.onabort = () => reject(transaction.error || new Error("identity_store_aborted"));
      });
    } finally {
      database.close();
    }
  }

  /** @param {unknown} value */
  set(value) {
    return this.write((store) => store.put(value, RECORD_KEY));
  }

  clear() {
    return this.write((store) => store.delete(RECORD_KEY));
  }
}

/** @param {unknown} value */
function validRecord(value) {
  if (!value || typeof value !== "object") return false;
  const record = /** @type {Record<string, any>} */ (value);
  const key = record.privateKey;
  return record.version === IDENTITY_VERSION
    && typeof record.publicKey === "string"
    && record.publicKey.length > 0
    && key?.type === "private"
    && key?.extractable === false
    && key?.algorithm?.name === "Ed25519"
    && Array.isArray(key?.usages)
    && key.usages.includes("sign");
}

/**
 * Owns the browser credential lifecycle. chrome.storage.local deliberately
 * holds only pairing/association metadata; private key material lives in IDB.
 * @param {{keyStore: {get(): Promise<unknown>, set(value: unknown): Promise<void>, clear(): Promise<void>}, storage: chrome.storage.StorageArea, subtle: SubtleCrypto}} dependencies
 */
export function createIdentityManager({keyStore, storage, subtle}) {
  /**
   * Every credential operation joins this queue before touching persistent
   * state. In particular, revocation cannot clear the store while an earlier
   * key creation is still able to commit afterward.
   * @type {Promise<void>}
   */
  let lifecycle = Promise.resolve();

  /** @template T @param {() => Promise<T>} operation @returns {Promise<T>} */
  function serialized(operation) {
    const result = lifecycle.then(operation, operation);
    lifecycle = result.then(() => undefined, () => undefined);
    return result;
  }

  async function clearAssociation() {
    await storage.remove([...LEGACY_KEYS, ...ASSOCIATION_KEYS]);
  }

  async function create() {
    const pair = await subtle.generateKey({name: "Ed25519"}, false, ["sign", "verify"]);
    const publicKey = encode(await subtle.exportKey("raw", pair.publicKey));
    const record = {version: IDENTITY_VERSION, publicKey, privateKey: pair.privateKey};
    await keyStore.set(record);
    return record;
  }

  async function ensureOnce() {
    const legacy = await storage.get(LEGACY_KEYS);
    if (legacy.privateKey || legacy.publicKey) {
      // Legacy JWK credentials were extractable and must not remain associated
      // with a browser id after rotation. Force an explicit operator re-pair.
      await keyStore.clear();
      await clearAssociation();
    }

    const current = await keyStore.get().catch(() => undefined);
    if (validRecord(current)) return /** @type {any} */ (current);

    // An unreadable or partially cloned record cannot authenticate reliably.
    // Remove its server association before replacing it so failure is closed.
    await keyStore.clear().catch(() => undefined);
    await clearAssociation();
    return create();
  }

  function ensure() {
    return serialized(ensureOnce);
  }

  /** @param {string} nonce */
  function sign(nonce) {
    return serialized(async () => {
      const current = await ensureOnce();
      return subtle.sign("Ed25519", current.privateKey, decode(nonce));
    });
  }

  function revoke() {
    return serialized(async () => {
      await keyStore.clear();
      await clearAssociation();
    });
  }

  return {ensure, sign, revoke};
}

/** @param {ArrayBuffer} bytes */
function encode(bytes) {
  return btoa(String.fromCharCode(...new Uint8Array(bytes)))
    .replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "");
}

/** @param {string} value */
function decode(value) {
  const normalized = value.replaceAll("-", "+").replaceAll("_", "/");
  const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, "=");
  return Uint8Array.from(atob(padded), (character) => character.charCodeAt(0));
}
