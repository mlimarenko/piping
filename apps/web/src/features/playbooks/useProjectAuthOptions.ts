import { useQuery } from "@tanstack/react-query";

import { usePermissions } from "@/shared/auth/permissionsContext";
import { AUTOMATION_SECRETS_QUERY_KEY, listAutomationSecrets, type AutomationSecret } from "@/features/automation-secrets/api";
import { listRecords, type ConnConfRecord } from "@/features/connections/connection-config/api";

export type ProjectAuthOptionGap = "forbidden" | "error" | null;

export type ProjectAuthOptions = {
  secrets: AutomationSecret[];
  secretsLoading: boolean;
  secretsGap: ProjectAuthOptionGap;
  sshRecords: ConnConfRecord[];
  sshRecordsLoading: boolean;
  sshRecordsGap: ProjectAuthOptionGap;
};

/** Named secrets and SSH connection records selectable as project Git credentials. */
export function useProjectAuthOptions(): ProjectAuthOptions {
  const { can } = usePermissions();
  const canReadSecrets = can("READ", "SECRET_VARIABLE");
  const canReadRecords = can("READ", "CONFIG_CONTEXT");
  const secrets = useQuery({ queryKey: AUTOMATION_SECRETS_QUERY_KEY, queryFn: listAutomationSecrets, enabled: canReadSecrets });
  const records = useQuery({
    queryKey: ["connection-config", "records", "project-auth"],
    queryFn: () => listRecords({ protocol: "ssh", enabled: "true" }),
    enabled: canReadRecords,
  });
  return {
    secrets: secrets.data ?? [],
    secretsLoading: secrets.isLoading,
    secretsGap: !canReadSecrets ? "forbidden" : secrets.isError ? "error" : null,
    sshRecords: records.data ?? [],
    sshRecordsLoading: records.isLoading,
    sshRecordsGap: !canReadRecords ? "forbidden" : records.isError ? "error" : null,
  };
}
