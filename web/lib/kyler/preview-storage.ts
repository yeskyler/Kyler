/** Artifact-local storage; never exposes the application's Storage object to an iframe. */
export function storageSnapshot(value: unknown): Record<string, string> | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const entries = Object.entries(value);
  if (entries.length > 256 || entries.some(([, item]) => typeof item !== "string")) return null;
  const result = Object.fromEntries(entries) as Record<string, string>;
  return JSON.stringify(result).length <= 65536 ? result : null;
}
export function previewStorageKey(session: string, file: string) {
  return `kyler.artifact.storage.${encodeURIComponent(session)}.${encodeURIComponent(file)}`;
}

/** Self-contained because this function is serialized into an opaque-origin iframe. */
export function installPreviewStorage(initial: Record<string, string>, channel: string) {
  function createStorage(seed: Record<string, string>, persist: boolean): Storage {
    const data = new Map(Object.entries(seed));
    const snapshot = () => Object.fromEntries(data);
    const changed = () => {
      if (persist)
        parent.postMessage({ type: "kyler-preview-storage", channel, value: snapshot() }, "*");
    };
    const put = (key: unknown, value: unknown) => {
      const name = String(key);
      const text = String(value);
      const next = new Map(data);
      next.set(name, text);
      if (next.size > 256 || JSON.stringify(Object.fromEntries(next)).length > 65536)
        throw new DOMException("Artifact storage quota exceeded.", "QuotaExceededError");
      data.set(name, text);
      changed();
    };
    const methods: Record<string, unknown> = {
      getItem: (key: unknown) => data.get(String(key)) ?? null,
      setItem: put,
      removeItem: (key: unknown) => {
        data.delete(String(key));
        changed();
      },
      clear: () => {
        data.clear();
        changed();
      },
      key: (index: number) => [...data.keys()][Number(index)] ?? null,
    };
    return new Proxy(Object.create(null), {
      get: (_, key) =>
        key === "length"
          ? data.size
          : key === Symbol.toStringTag
            ? "Storage"
            : typeof key === "string" && Object.prototype.hasOwnProperty.call(methods, key)
              ? methods[key]
              : typeof key === "string"
                ? data.get(key)
                : undefined,
      set: (_, key, value) => {
        if (typeof key === "string") put(key, value);
        return true;
      },
      deleteProperty: (_, key) => {
        if (typeof key === "string") {
          data.delete(key);
          changed();
        }
        return true;
      },
      ownKeys: () => [...data.keys()],
      has: (_, key) =>
        typeof key === "string" &&
        (data.has(key) || key === "length" || Object.prototype.hasOwnProperty.call(methods, key)),
      getOwnPropertyDescriptor: (_, key) =>
        typeof key === "string" && data.has(key)
          ? { value: data.get(key), writable: true, enumerable: true, configurable: true }
          : undefined,
    }) as Storage;
  }
  Object.defineProperty(window, "localStorage", {
    value: createStorage(initial, true),
    configurable: true,
  });
  Object.defineProperty(window, "sessionStorage", {
    value: createStorage({}, false),
    configurable: true,
  });
}
