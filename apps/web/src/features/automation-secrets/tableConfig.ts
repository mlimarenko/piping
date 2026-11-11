/**
 * Table contract for the automation-secrets DataTable.
 *
 * The list endpoint returns the full (small) secret set without pagination,
 * so sorting is applied client-side; this module pins the sortable column
 * keys the screen may hand to `sanitizeDataTableSortState`.
 */
interface AutomationSecretsTableConfig {
  sortKeys: string[];
}

export const AUTOMATION_SECRETS_TABLE_CONFIG: AutomationSecretsTableConfig = {
  sortKeys: ["key", "description", "has_value", "usage_count", "created_at"],
};
