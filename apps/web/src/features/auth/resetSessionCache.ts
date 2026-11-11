import type { QueryClient } from "@tanstack/react-query";
import { BRANDING_QUERY_KEY } from "@/shared/api/branding-api";

/** Remove user-scoped cache while retaining the one public instance query and its observers. */
export function resetSessionCache(client: QueryClient) {
  client.removeQueries({ predicate: query =>
    query.queryKey.length !== BRANDING_QUERY_KEY.length ||
    !BRANDING_QUERY_KEY.every((part, index) => query.queryKey[index] === part),
  });
  client.getMutationCache().clear();
}
