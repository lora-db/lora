import React from "react";

/**
 * Mirrors the Docusaurus navbar's `hideOnScroll` state on a sticky
 * in-page strip. Returns a ref to attach to the strip element and a
 * boolean — when true, the strip should translate up by the navbar's
 * height so the two surfaces move as one (instead of leaving an
 * empty band above the strip when the navbar slides away).
 *
 * The logic is the same as the homepage / features page anchor strip:
 *
 *   - Lift only when the strip is sticky-pinned AND the user is
 *     scrolling down. Below the pin threshold the strip lives in the
 *     hero in normal flow — never lift it then.
 *   - Reveal on any upward scroll, or near the top of the page.
 *   - Use `getBoundingClientRect` + offsetParent walk to find the
 *     strip's natural (un-transformed) document Y, so the check
 *     stays correct even while the strip is currently lifted.
 */
export default function useNavbarHide() {
  const stripRef = React.useRef(null);
  const [hidden, setHidden] = React.useState(false);

  React.useEffect(() => {
    let frame = 0;
    let lastY = window.scrollY;
    let lifted = false;
    const TOP_REVEAL_THRESHOLD = 60;

    const getAbsTop = (el) => {
      let y = 0;
      let cur = el;
      while (cur) {
        y += cur.offsetTop;
        cur = cur.offsetParent;
      }
      return y;
    };

    const update = () => {
      frame = 0;
      const navbar = document.querySelector(".navbar");
      const strip = stripRef.current;
      if (!navbar || !strip) {
        setHidden(false);
        return;
      }
      const y = window.scrollY;
      const navHeight = navbar.offsetHeight || 60;
      const stripIsPinned = y >= getAbsTop(strip) - navHeight;
      const goingUp = y < lastY;
      const goingDown = y > lastY;

      if (goingUp || y < TOP_REVEAL_THRESHOLD) {
        lifted = false;
      } else if (goingDown && stripIsPinned) {
        lifted = true;
      }
      setHidden(lifted);
      lastY = y;
    };

    const onScroll = () => {
      if (frame) return;
      frame = window.requestAnimationFrame(update);
    };

    update();
    window.addEventListener("scroll", onScroll, { passive: true });
    window.addEventListener("resize", onScroll);
    return () => {
      if (frame) cancelAnimationFrame(frame);
      window.removeEventListener("scroll", onScroll);
      window.removeEventListener("resize", onScroll);
    };
  }, []);

  return [stripRef, hidden];
}
