import { useEffect, useState } from "react";

/** Track whether Shift is currently held. In 3D mode this lets us
 *  disable the Three.js navigation controls while the user is drawing a
 *  marquee — otherwise OrbitControls / TrackballControls would also
 *  process the same drag and pan the camera. */
export function useShiftHeld(): boolean {
  const [shiftHeld, setShiftHeld] = useState(false);
  useEffect(() => {
    const onDown = (e: KeyboardEvent) => {
      if (e.key === "Shift") setShiftHeld(true);
    };
    const onUp = (e: KeyboardEvent) => {
      if (e.key === "Shift") setShiftHeld(false);
    };
    const onBlur = () => setShiftHeld(false);
    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
      window.removeEventListener("blur", onBlur);
    };
  }, []);
  return shiftHeld;
}
