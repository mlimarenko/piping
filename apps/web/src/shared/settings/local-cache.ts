type Normalizer<T> = (value: unknown) => T;

interface SettingCacheEnvelope {
  version: 1;
  key: string;
  value: unknown;
  updatedAt: string;
}

const SETTING_CACHE_PREFIX = "piping.settings.cache.v1:";
const LOCAL_STATE_PREFIX = "piping.localState.v1:";
const SESSION_STATE_PREFIX = "piping.sessionState.v1:";

function readStorage(storage: Storage | undefined, key: string): string | null {
  if (!storage) return null;
  try {
    return storage.getItem(key);
  } catch {
    return null;
  }
}

function writeStorage(storage: Storage | undefined, key: string, value: string): void {
  if (!storage) return;
  try {
    storage.setItem(key, value);
  } catch {
    /* storage unavailable */
  }
}

function removeStorage(storage: Storage | undefined, key: string): void {
  if (!storage) return;
  try {
    storage.removeItem(key);
  } catch {
    /* storage unavailable */
  }
}

function local(): Storage | undefined {
  return typeof window === "undefined" ? undefined : window.localStorage;
}

function session(): Storage | undefined {
  return typeof window === "undefined" ? undefined : window.sessionStorage;
}

function parseJson(raw: string | null): unknown {
  if (!raw) return undefined;
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    return undefined;
  }
}

export function readCachedSettingValue<T>(
  key: string,
  normalize: Normalizer<T>,
): T | undefined {
  const parsed = parseJson(readStorage(local(), `${SETTING_CACHE_PREFIX}${key}`));
  if (
    typeof parsed !== "object" ||
    parsed === null ||
    (parsed as Partial<SettingCacheEnvelope>).version !== 1 ||
    (parsed as Partial<SettingCacheEnvelope>).key !== key
  ) {
    return undefined;
  }
  return normalize((parsed as SettingCacheEnvelope).value);
}

export function writeCachedSettingValue(key: string, value: unknown): void {
  const envelope: SettingCacheEnvelope = {
    version: 1,
    key,
    value,
    updatedAt: new Date().toISOString(),
  };
  writeStorage(local(), `${SETTING_CACHE_PREFIX}${key}`, JSON.stringify(envelope));
}

export function clearCachedSettingValue(key: string): void {
  removeStorage(local(), `${SETTING_CACHE_PREFIX}${key}`);
}

export function readRetiredLocalValue<T>(
  retiredKey: string,
  normalize: Normalizer<T>,
): T | undefined {
  const raw = readStorage(local(), retiredKey);
  if (raw === null) return undefined;
  return normalize(raw);
}

export function readRetiredLocalJson<T>(
  retiredKey: string,
  normalize: Normalizer<T>,
): T | undefined {
  const parsed = parseJson(readStorage(local(), retiredKey));
  return parsed === undefined ? undefined : normalize(parsed);
}

export function retireLocalKey(retiredKey: string): void {
  removeStorage(local(), retiredKey);
}

export function readBrowserLocalSetting<T>(
  key: string,
  defaultValue: T,
  normalize: Normalizer<T>,
): T {
  const parsed = parseJson(readStorage(local(), `${LOCAL_STATE_PREFIX}${key}`));
  return parsed === undefined ? defaultValue : normalize(parsed);
}

export function writeBrowserLocalSetting<T>(
  key: string,
  value: T,
  normalize: Normalizer<T> = (next) => next as T,
): void {
  writeStorage(local(), `${LOCAL_STATE_PREFIX}${key}`, JSON.stringify(normalize(value)));
}

export function removeBrowserLocalSetting(key: string): void {
  removeStorage(local(), `${LOCAL_STATE_PREFIX}${key}`);
}

export function readSessionSetting<T>(
  key: string,
  defaultValue: T,
  normalize: Normalizer<T>,
): T {
  const parsed = parseJson(readStorage(session(), `${SESSION_STATE_PREFIX}${key}`));
  return parsed === undefined ? defaultValue : normalize(parsed);
}

export function writeSessionSetting<T>(
  key: string,
  value: T,
  normalize: Normalizer<T> = (next) => next as T,
): void {
  writeStorage(session(), `${SESSION_STATE_PREFIX}${key}`, JSON.stringify(normalize(value)));
}

export function removeSessionSetting(key: string): void {
  removeStorage(session(), `${SESSION_STATE_PREFIX}${key}`);
}

export function readRetiredSessionJson<T>(
  retiredKey: string,
  normalize: Normalizer<T>,
): T | undefined {
  const parsed = parseJson(readStorage(session(), retiredKey));
  return parsed === undefined ? undefined : normalize(parsed);
}

