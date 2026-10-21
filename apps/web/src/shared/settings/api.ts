import { apiClient, unwrap } from "@/shared/api/client";
import type { components } from "@/shared/api/schema.gen";

export type EffectiveSettingDto = components["schemas"]["EffectiveSetting"];
export type SettingOverrideDto = components["schemas"]["SettingOverride"];
export type SecretFieldStateDto = components["schemas"]["SecretFieldStateResponse"];

export interface SecretFieldTarget {
  consumerType: string;
  consumerId: string;
  ownerScopeType: string;
  ownerScopeId: string;
  fieldKey: string;
}

export async function fetchEffectiveSettings(keys?: string[]): Promise<EffectiveSettingDto[]> {
  const params = keys && keys.length > 0 ? { query: { keys: keys.join(",") } } : {};
  const response = unwrap(
    await apiClient.GET("/api/v1/settings/effective", {
      params,
    }),
  );
  return response.settings;
}

export async function fetchEffectiveSetting(key: string): Promise<EffectiveSettingDto> {
  return unwrap(
    await apiClient.GET("/api/v1/settings/effective/{key}", {
      params: { path: { key } },
    }),
  );
}

export async function upsertUserSetting(key: string, value: unknown): Promise<void> {
  await apiClient.PUT("/api/v1/settings/user/{key}", {
    params: { path: { key } },
    body: { value },
  });
}

export async function deleteUserSetting(key: string): Promise<void> {
  await apiClient.DELETE("/api/v1/settings/user/{key}", {
    params: { path: { key } },
  });
}

export async function fetchAdminSettingDefaults(): Promise<SettingOverrideDto[]> {
  const response = unwrap(await apiClient.GET("/api/v1/admin/settings/defaults", {}));
  return response.settings;
}

export async function upsertAdminSettingDefault(
  key: string,
  value: unknown,
): Promise<SettingOverrideDto> {
  return unwrap(
    await apiClient.PUT("/api/v1/admin/settings/defaults/{key}", {
      params: { path: { key } },
      body: { value },
    }),
  );
}

export async function deleteAdminSettingDefault(key: string): Promise<void> {
  await apiClient.DELETE("/api/v1/admin/settings/defaults/{key}", {
    params: { path: { key } },
  });
}

export async function fetchSecretFieldStates(
  consumerType: string,
  consumerId: string,
): Promise<SecretFieldStateDto[]> {
  return unwrap(
    await apiClient.GET("/api/v1/secret-fields", {
      params: { query: { consumer_type: consumerType, consumer_id: consumerId } },
    }),
  );
}

export async function setSecretField(
  target: SecretFieldTarget,
  value: string,
): Promise<SecretFieldStateDto> {
  return unwrap(
    await apiClient.PUT("/api/v1/secret-fields", {
      body: {
        consumer_type: target.consumerType,
        consumer_id: target.consumerId,
        owner_scope_type: target.ownerScopeType,
        owner_scope_id: target.ownerScopeId,
        field_key: target.fieldKey,
        intent: "set",
        value,
      },
    }),
  );
}

export async function clearSecretField(target: SecretFieldTarget): Promise<SecretFieldStateDto> {
  return unwrap(
    await apiClient.PUT("/api/v1/secret-fields", {
      body: {
        consumer_type: target.consumerType,
        consumer_id: target.consumerId,
        owner_scope_type: target.ownerScopeType,
        owner_scope_id: target.ownerScopeId,
        field_key: target.fieldKey,
        intent: "clear",
        value: null,
      },
    }),
  );
}
