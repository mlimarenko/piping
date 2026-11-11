-- "Remember this device" browser sessions (PIP-609): a session opened with the
-- explicit opt-in stores its absolute deadline, so refresh keeps it alive
-- without ever moving it further out. Existing and ordinary sessions keep
-- NULL, which preserves the current inactivity-TTL behaviour with no backfill.
ALTER TABLE public.auth_session
    ADD COLUMN remember_until timestamp with time zone;

-- Invariant the refresh path relies on: a remembered session's expiry never
-- passes its own deadline. Ordinary sessions (NULL) are unrestricted.
ALTER TABLE public.auth_session
    ADD CONSTRAINT auth_session_remember_until_bounds
    CHECK (remember_until IS NULL OR expires_at <= remember_until);
