import { useEffect, useState } from "react";

/** Track the user's `prefers-reduced-motion` media query. Returns
 *  `true` when the user has asked the OS to minimise non-essential
 *  motion — our camera tweens (intro zoom, mode transition, focus
 *  fly-in) skip the animation in that case and snap directly to the
 *  final state.
 *
 *  Re-reads on media-query change so we react when the user flips the
 *  setting mid-session. Returns `false` in non-browser environments
 *  (SSR / jsdom without matchMedia) so animations play by default. */
export function usePrefersReducedMotion(): boolean {
  const [reduce, setReduce] = useState(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
      return false;
    }
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  });

  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
      return;
    }
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const onChange = () => setReduce(mq.matches);
    // addEventListener is the modern shape; addListener is the legacy
    // shape kept for older Safari builds. Either / or, never both.
    if (typeof mq.addEventListener === "function") {
      mq.addEventListener("change", onChange);
      return () => mq.removeEventListener("change", onChange);
    }
    mq.addListener(onChange);
    return () => mq.removeListener(onChange);
  }, []);

  return reduce;
}
