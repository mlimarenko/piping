import { useCallback, useEffect, useMemo, useRef } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  deleteAdminSettingDefault,
  deleteUserSetting,
  fetchAdminSettingDefaults,
  fetchEffectiveSetting,
  fetchEffectiveSettings,
  upsertAdminSettingDefault,
  upsertUserSetting,
  type EffectiveSettingDto,
  type SettingOverrideDto,
} from "./api";
import {
  getServerSettingDefinition,
  type SettingDefinition,
} from "./registry";
import {
  clearCachedSettingValue,
  readCachedSettingValue,
  writeCachedSettingValue,
} from "./local-cache";

type Normalizer<T> = (value: unknown) => T;
type SettingSnapshot = {
  existed: boolean;
  data?: EffectiveSettingDto;
};

export interface UseSettingResult<T> {
  value: T;
  /**
   * Optimistically applies the value immediately. The returned promise resolves
   * `true` only after the server accepts the latest write and `false` when that
   * write fails or a newer write supersedes it. It never rejects, so existing
   * fire-and-forget callers need no catch handler.
   */
  setValue: (next: T | ((prev: T) => T)) => Promise<boolean>;
  /** Same completion contract as {@link setValue}, for deleting the override. */
  resetValue: () => Promise<boolean>;
  isLoading: boolean;
  isFetching: boolean;
  source: EffectiveSettingDto["source"] | "default";
  hasUserOverride: boolean;
}

export interface UseSettingOptions<T> {
  debounceMs?: number;
  enabled?: boolean;
  normalize?: Normalizer<T>;
  bootstrapCache?: boolean;
}

function effectiveQueryKey(key: string) {
  return ["settings", "effective", key] as const;
}

function identityNormalizer<T>(value: unknown): T {
  return value as T;
}

function defaultSetting<T>(key: string, fallback: T): {
  definition?: SettingDefinition;
  value: T;
  normalize: Normalizer<T>;
  bootstrapCache: boolean;
} {
  const definition = getServerSettingDefinition(key);
  return {
    definition,
    value: definition ? (definition.defaultValue as T) : fallback,
    normalize: definition ? (definition.normalize as Normalizer<T>) : identityNormalizer<T>,
    bootstrapCache: definition?.cachePolicy === "bootstrap",
  };
}

function normalizeEffective<T>(
  setting: EffectiveSettingDto | undefined,
  fallback: T,
  normalize: Normalizer<T>,
): T {
  return normalize(setting?.value ?? fallback);
}

function captureSettingSnapshot(
  qc: ReturnType<typeof useQueryClient>,
  queryKey: readonly unknown[],
): SettingSnapshot {
  const state = qc.getQueryState<EffectiveSettingDto>(queryKey);
  return {
    existed: state?.data !== undefined,
    data: state?.data,
  };
}

function restoreSettingSnapshot(
  qc: ReturnType<typeof useQueryClient>,
  queryKey: readonly unknown[],
  snapshot: SettingSnapshot,
) {
  if (snapshot.existed) {
    qc.setQueryData(queryKey, snapshot.data);
  } else {
    qc.removeQueries({ queryKey, exact: true });
  }
}

function syncCachedSettingValue<T>(
  key: string,
  setting: EffectiveSettingDto | undefined,
  normalize: Normalizer<T>,
) {
  if (!setting || setting.source === "unset" || setting.value === undefined) {
    clearCachedSettingValue(key);
    return;
  }
  writeCachedSettingValue(key, normalize(setting.value));
}

function optimisticUserSetting<T>(
  key: string,
  value: T,
  snapshot: SettingSnapshot,
): EffectiveSettingDto {
  return {
    key,
    value,
    source: "user",
    user_value: value,
    instance_value: snapshot.data?.instance_value,
    updated_at: new Date().toISOString(),
  };
}

function optimisticResetSetting(key: string, snapshot: SettingSnapshot): EffectiveSettingDto {
  const instanceValue = snapshot.data?.instance_value;
  return {
    key,
    value: instanceValue,
    source: instanceValue === undefined ? "unset" : "instance",
    user_value: undefined,
    instance_value: instanceValue,
    updated_at: snapshot.data?.updated_at,
  };
}

export function useSetting<T>(
  key: string,
  fallbackValue: T,
  options?: UseSettingOptions<T>,
): UseSettingResult<T> {
  const qc = useQueryClient();
  const queryKey = useMemo(() => effectiveQueryKey(key), [key]);
  const debounceMs = options?.debounceMs ?? 0;
  const enabled = options?.enabled ?? true;
  const registry = defaultSetting(key, fallbackValue);
  const fallback = registry.value;
  const normalize = options?.normalize ?? registry.normalize;
  const useBootstrapCache = options?.bootstrapCache ?? registry.bootstrapCache;
  const debounceTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingDebounceSnapshot = useRef<SettingSnapshot | null>(null);
  const pendingDebounceResolve = useRef<((saved: boolean) => void) | null>(null);
  const debounceSequence = useRef(0);
  const latestWriteSequence = useRef(0);
  const writeQueue = useRef<Promise<void>>(Promise.resolve());

  const settlePendingDebounce = useCallback((saved: boolean) => {
    const resolve = pendingDebounceResolve.current;
    pendingDebounceResolve.current = null;
    resolve?.(saved);
  }, []);

  const clearPendingDebounce = useCallback(() => {
    debounceSequence.current += 1;
    if (debounceTimer.current !== null) {
      clearTimeout(debounceTimer.current);
      debounceTimer.current = null;
    }
    pendingDebounceSnapshot.current = null;
    settlePendingDebounce(false);
  }, [settlePendingDebounce]);

  useEffect(() => {
    return clearPendingDebounce;
  }, [clearPendingDebounce]);

  const query = useQuery({
    queryKey,
    enabled,
    queryFn: () => fetchEffectiveSetting(key),
    retry: false,
  });

  const enqueueWrite = useCallback((task: () => Promise<unknown>) => {
    const runTask = () => task().then(() => undefined);
    const next = writeQueue.current.then(runTask, runTask);
    writeQueue.current = next.catch(() => undefined);
    return next;
  }, []);

  // Stable normalized values prevent unrelated renders from rehydrating consumers.
  // Bootstrap cache applies only until the server answers, including `unset`.
  const cachedFallback = useMemo(
    () =>
      !query.data && useBootstrapCache
        ? (readCachedSettingValue(key, normalize) ?? fallback)
        : fallback,
    [fallback, key, normalize, query.data, useBootstrapCache],
  );

  const value = useMemo(
    () => normalizeEffective(query.data, cachedFallback, normalize),
    [cachedFallback, normalize, query.data],
  );

  useEffect(() => {
    if (!useBootstrapCache || !query.data) return;
    syncCachedSettingValue(key, query.data, normalize);
  }, [key, useBootstrapCache, query.data, normalize]);

  const setValue = useCallback(
    (next: T | ((prev: T) => T)) => {
      const current = normalizeEffective(qc.getQueryData(queryKey), cachedFallback, normalize);
      const resolved = typeof next === "function" ? (next as (prev: T) => T)(current) : next;
      const normalized = normalize(resolved);
      const snapshot = captureSettingSnapshot(qc, queryKey);
      const mutationSnapshot =
        debounceMs > 0
          ? (pendingDebounceSnapshot.current ?? snapshot)
          : snapshot;
      latestWriteSequence.current += 1;
      const writeSequence = latestWriteSequence.current;
      void qc.cancelQueries({ queryKey });
      qc.setQueryData<EffectiveSettingDto>(
        queryKey,
        optimisticUserSetting(key, normalized, mutationSnapshot),
      );
      if (useBootstrapCache) writeCachedSettingValue(key, normalized);
      const persist = (): Promise<boolean> =>
        enqueueWrite(() => upsertUserSetting(key, normalized))
          .then(() => writeSequence === latestWriteSequence.current)
          .catch(() => {
            if (writeSequence === latestWriteSequence.current) {
              restoreSettingSnapshot(qc, queryKey, mutationSnapshot);
              if (useBootstrapCache) {
                syncCachedSettingValue(key, mutationSnapshot.data, normalize);
              }
            }
            return false;
          })
          .finally(() => {
            if (writeSequence !== latestWriteSequence.current) return;
            void qc.invalidateQueries({ queryKey });
          });

      if (debounceMs > 0) {
        if (debounceTimer.current !== null) clearTimeout(debounceTimer.current);
        settlePendingDebounce(false);
        debounceSequence.current += 1;
        const scheduledDebounceSequence = debounceSequence.current;
        pendingDebounceSnapshot.current = mutationSnapshot;
        return new Promise<boolean>((resolve) => {
          pendingDebounceResolve.current = resolve;
          debounceTimer.current = setTimeout(() => {
            if (debounceSequence.current !== scheduledDebounceSequence) {
              resolve(false);
              return;
            }
            debounceTimer.current = null;
            pendingDebounceSnapshot.current = null;
            pendingDebounceResolve.current = null;
            void persist().then(resolve);
          }, debounceMs);
        });
      }
      return persist();
    },
    [
      cachedFallback,
      debounceMs,
      enqueueWrite,
      key,
      normalize,
      qc,
      queryKey,
      settlePendingDebounce,
      useBootstrapCache,
    ],
  );

  const resetValue = useCallback(() => {
    clearPendingDebounce();
    latestWriteSequence.current += 1;
    const writeSequence = latestWriteSequence.current;
    const snapshot = captureSettingSnapshot(qc, queryKey);
    const resetSetting = optimisticResetSetting(key, snapshot);
    void qc.cancelQueries({ queryKey });
    qc.setQueryData(queryKey, resetSetting);

    return enqueueWrite(() => deleteUserSetting(key))
      .then(() => {
        if (writeSequence !== latestWriteSequence.current) return false;
        qc.setQueryData(queryKey, resetSetting);
        if (!useBootstrapCache) {
          clearCachedSettingValue(key);
        } else {
          syncCachedSettingValue(key, resetSetting, normalize);
        }
        return true;
      })
      .catch(() => {
        if (writeSequence === latestWriteSequence.current) {
          restoreSettingSnapshot(qc, queryKey, snapshot);
          if (useBootstrapCache) {
            syncCachedSettingValue(key, snapshot.data, normalize);
          }
        }
        return false;
      })
      .finally(() => {
        void qc.invalidateQueries({ queryKey });
      });
  }, [
    clearPendingDebounce,
    enqueueWrite,
    key,
    normalize,
    qc,
    queryKey,
    useBootstrapCache,
  ]);

  return {
    value,
    setValue,
    resetValue,
    isLoading: query.isLoading,
    isFetching: query.isFetching,
    source: query.data?.source ?? "default",
    hasUserOverride: query.data?.source === "user",
  };
}

export function useEffectiveSettings(keys?: string[]) {
  return useQuery({
    queryKey: ["settings", "effective", keys ?? "all"],
    queryFn: () => fetchEffectiveSettings(keys),
    staleTime: 30_000,
  });
}

export function useAdminSettingDefaults(options?: { enabled?: boolean }) {
  const qc = useQueryClient();
  const queryKey = ["settings", "admin-defaults"] as const;
  const query = useQuery({
    queryKey,
    queryFn: fetchAdminSettingDefaults,
    staleTime: 30_000,
    enabled: options?.enabled ?? true,
  });
  const upsert = useMutation({
    mutationFn: ({ key, value }: { key: string; value: unknown }) =>
      upsertAdminSettingDefault(key, value),
    onSuccess: (saved: SettingOverrideDto) => {
      qc.setQueryData<SettingOverrideDto[]>(queryKey, (current = []) => {
        const without = current.filter((item) => item.key !== saved.key);
        return [...without, saved].sort((a, b) => a.key.localeCompare(b.key));
      });
      void qc.invalidateQueries({ queryKey: ["settings"] });
    },
  });
  const remove = useMutation({
    mutationFn: deleteAdminSettingDefault,
    onSuccess: (_data, key) => {
      qc.setQueryData<SettingOverrideDto[]>(queryKey, (current = []) =>
        current.filter((item) => item.key !== key),
      );
      void qc.invalidateQueries({ queryKey: ["settings"] });
    },
  });

  return {
    ...query,
    upsertDefault: upsert.mutate,
    deleteDefault: remove.mutate,
    isSaving: upsert.isPending || remove.isPending,
  };
}
