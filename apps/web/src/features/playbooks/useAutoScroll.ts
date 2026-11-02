import { useCallback, useEffect, useRef, useState } from "react";

/**
 * Auto-scroll hook for a streaming output container.
 *
 * While `userScrolled` is false the container is pinned to the bottom
 * whenever `dependency` changes (i.e. when new events arrive — callers pass
 * the rendered event count). Scrolling up sets the flag; scrolling back to
 * within 48px of the bottom clears it again.
 */
export function useAutoScroll<T extends HTMLElement>(dependency: unknown) {
  const containerRef = useRef<T>(null);
  const [userScrolled, setUserScrolled] = useState(false);
  const userScrolledRef = useRef(userScrolled);

  // Mirror the flag into a ref so the pin effect below depends only on
  // `dependency`, matching the original "pin when new content arrives" contract.
  useEffect(() => {
    userScrolledRef.current = userScrolled;
  }, [userScrolled]);

  // Pin to bottom when new content arrives and the user hasn't scrolled up.
  useEffect(() => {
    const el = containerRef.current;
    if (!el || userScrolledRef.current) return;
    el.scrollTop = el.scrollHeight;
  }, [dependency]);

  const onScroll = useCallback((e: React.UIEvent<HTMLElement>) => {
    const el = e.currentTarget;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    setUserScrolled(!atBottom);
  }, []);

  const scrollToBottom = useCallback(() => {
    const el = containerRef.current;
    if (el) el.scrollTop = el.scrollHeight;
    setUserScrolled(false);
  }, []);

  return { containerRef, userScrolled, onScroll, scrollToBottom };
}
