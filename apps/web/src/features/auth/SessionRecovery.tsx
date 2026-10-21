import { useEffect } from "react";
import { useLocation, useNavigate } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { me } from "./api";
import { ApiError, refreshSession, subscribeSessionExpired } from "@/shared/api/client";

/** Mounted once for the shell and detached views, not once per failing table. */
export default function SessionRecovery() {
  const location = useLocation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const protectedPage = !location.pathname.startsWith("/auth/");
  const auth = useQuery({
    queryKey: ["auth", "me"], queryFn: me, enabled: protectedPage,
    retry: false, refetchOnWindowFocus: true,
  });
  const refetch = auth.refetch;

  useEffect(() => {
    if (!protectedPage) return;
    const expire = () => {
      const redirectTo = location.pathname + location.search + location.hash;
      void navigate(`/auth/login?${new URLSearchParams({ redirect_to: redirectTo })}`, { replace: true });
      queryClient.clear();
    };
    const unsubscribe = subscribeSessionExpired(expire);
    if (auth.error instanceof ApiError && auth.error.status === 401) expire();
    return unsubscribe;
  }, [auth.error, location.pathname, location.search, location.hash, navigate, protectedPage, queryClient]);

  useEffect(() => {
    if (!protectedPage || !auth.data?.expiresAt) return;
    const expiry = Date.parse(auth.data.expiresAt);
    if (!Number.isFinite(expiry)) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const renew = async () => {
      try {
        if (await refreshSession() && !stopped) await refetch({ throwOnError: true });
      } catch {
        // A deployment/network failure is not a logout. Retry before the next
        // user request; the API boundary still handles definitive expiry.
        if (!stopped) timer = setTimeout(() => { void renew(); }, 30_000);
      }
    };
    const remaining = expiry - Date.now();
    timer = setTimeout(() => { void renew(); }, Math.min(2_147_483_647, Math.max(0, remaining - Math.min(60_000, remaining / 2))));
    const onFocus = () => { if (Date.now() >= expiry - 60_000) { clearTimeout(timer); void renew(); } };
    window.addEventListener("focus", onFocus);
    return () => { stopped = true; clearTimeout(timer); window.removeEventListener("focus", onFocus); };
  }, [auth.data?.expiresAt, refetch, protectedPage]);

  return null;
}
