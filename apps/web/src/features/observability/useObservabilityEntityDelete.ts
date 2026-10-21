import { useState } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";
import { useApiMutation } from "@/shared/api/hooks";
import type { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";
import type { EntityDetailAction } from "@/shared/ui-shared/EntityDetail";

interface UseObservabilityEntityDeleteOptions {
  /** Deletes one entity by id; resolves on success. */
  deleteFn: (id: string) => Promise<void>;
  /** Cache invalidation run after a successful delete, before navigation. */
  invalidate: () => void;
  /** List route navigated back to after a successful delete. */
  listHref: string;
}

/**
 * Feature-owned delete lifecycle for the four observability entities. The shared
 * `EntityDetail` frame stays pure: this hook owns the mutation, pending/error
 * state, success notification and navigation, and hands the frame a pre-wired
 * confirmation dialog via {@link deleteConfirm}.
 */
export function useObservabilityEntityDelete({
  deleteFn,
  invalidate,
  listHref,
}: UseObservabilityEntityDeleteOptions) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [deleteError, setDeleteError] = useState("");

  const mutation = useApiMutation<void, string>({
    // Wrap so TanStack's extra `mutationFnContext` argument never leaks into the
    // domain delete function, which accepts exactly one `id` parameter.
    mutationFn: (id: string) => deleteFn(id),
    onSuccess: () => {
      invalidate();
      notify("success", t("observability.entityDeleted", { defaultValue: "Deleted" }));
      void navigate(listHref);
    },
    onError: (err: ApiError) =>
      setDeleteError(
        err.message || t("observability.entityDeleteFailed", { defaultValue: "Failed to delete" }),
      ),
  });

  return {
    deleteError,
    isDeleting: mutation.isPending,
    deleteConfirm: (name: string, id: string): NonNullable<EntityDetailAction["confirm"]> => ({
      title: t("delete"),
      description: t("observability.entityDeleteConfirm", {
        name,
        defaultValue: 'Delete "{{name}}"? This cannot be undone.',
      }),
      onConfirm: () => mutation.mutate(id),
    }),
  };
}
