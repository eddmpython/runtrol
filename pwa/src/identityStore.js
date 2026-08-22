const DATABASE = "runtrol-phone";
const VERSION = 1;
const STORE = "device";
const IDENTITY = "identity";
const CONNECTION = "connection";

export async function openDeviceStore(indexedDb = indexedDB) {
  const database = await new Promise((resolve, reject) => {
    const request = indexedDb.open(DATABASE, VERSION);
    request.onupgradeneeded = () => {
      if (!request.result.objectStoreNames.contains(STORE)) {
        request.result.createObjectStore(STORE);
      }
    };
    request.onerror = () => reject(new Error("device storage could not be opened", { cause: request.error }));
    request.onsuccess = () => resolve(request.result);
  });
  return new DeviceStore(database);
}

export class DeviceStore {
  constructor(database) {
    this.database = database;
  }

  async identity() {
    const stored = await this.get(IDENTITY);
    if (stored !== undefined) return validateIdentity(stored);
    const pair = await crypto.subtle.generateKey({ name: "X25519" }, false, ["deriveBits"]);
    if (pair.privateKey.extractable || !pair.publicKey.extractable) {
      throw new Error("device identity key extractability is unsafe");
    }
    const identity = { privateKey: pair.privateKey, publicKey: pair.publicKey };
    await this.put(IDENTITY, identity);
    return identity;
  }

  async connection() {
    const stored = await this.get(CONNECTION);
    return stored === undefined ? null : validateConnection(stored);
  }

  async saveConnection(connection) {
    await this.put(CONNECTION, validateConnection(connection));
  }

  async forget() {
    await transactionRequest(this.database, "readwrite", (store) => store.clear());
  }

  close() {
    this.database.close();
  }

  get(key) {
    return transactionRequest(this.database, "readonly", (store) => store.get(key));
  }

  put(key, value) {
    return transactionRequest(this.database, "readwrite", (store) => store.put(value, key));
  }
}

function transactionRequest(database, mode, action) {
  return new Promise((resolve, reject) => {
    const transaction = database.transaction(STORE, mode);
    const request = action(transaction.objectStore(STORE));
    request.onerror = () => reject(new Error("device storage request failed", { cause: request.error }));
    transaction.onabort = () => reject(new Error("device storage transaction aborted", { cause: transaction.error }));
    transaction.oncomplete = () => resolve(request.result);
  });
}

function validateIdentity(value) {
  if (
    value === null
    || typeof value !== "object"
    || !(value.privateKey instanceof CryptoKey)
    || !(value.publicKey instanceof CryptoKey)
    || value.privateKey.type !== "private"
    || value.privateKey.extractable
    || value.privateKey.algorithm.name !== "X25519"
    || value.publicKey.type !== "public"
    || value.publicKey.algorithm.name !== "X25519"
  ) {
    throw new Error("stored device identity is invalid");
  }
  return value;
}

export function validateConnection(value) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("stored connection is invalid");
  }
  const legacyFields = [
    "deviceCredential",
    "pcPublicKey",
    "relayOrigin",
    "route",
    "routeCredential",
    "scopes",
  ];
  const authorityFields = [...legacyFields, "roots", "providers"].sort();
  const fields = [...authorityFields, "missionSignalCursor"].sort();
  const keys = Object.keys(value).sort();
  const legacy = JSON.stringify(keys) === JSON.stringify(legacyFields);
  const authorityOnly = JSON.stringify(keys) === JSON.stringify(authorityFields);
  if (!legacy && !authorityOnly && JSON.stringify(keys) !== JSON.stringify(fields)) {
    throw new Error("stored connection has an unexpected field set");
  }
  for (const field of legacyFields.slice(0, 5)) {
    if (typeof value[field] !== "string" || value[field].length === 0) {
      throw new Error(`stored connection ${field} is invalid`);
    }
  }
  for (const field of ["scopes", "roots", "providers"]) {
    const entries = legacy && field !== "scopes" ? [] : value[field];
    if (!Array.isArray(entries) || entries.some((entry) => typeof entry !== "string")) {
      throw new Error(`stored connection ${field} are invalid`);
    }
  }
  if (
    !legacy
    && !authorityOnly
    && value.missionSignalCursor !== null
    && (typeof value.missionSignalCursor !== "string" || !/^[0-9a-f]{32}$/u.test(value.missionSignalCursor))
  ) {
    throw new Error("stored connection Mission Flight Signal cursor is invalid");
  }
  return Object.freeze({
    ...value,
    scopes: Object.freeze([...value.scopes]),
    roots: Object.freeze([...(value.roots ?? [])]),
    providers: Object.freeze([...(value.providers ?? [])]),
    missionSignalCursor: value.missionSignalCursor ?? null,
  });
}
