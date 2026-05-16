import { useMemo } from "react";
import { perfTierDefaults, pickPerfTier } from "../utils/perfTier";
import type {
  GraphMode,
  LinkObject,
  LoraGraphCanvasProps,
  NodeObject,
} from "../types";

export interface UsePerfTierDefaultsParams {
  profile: LoraGraphCanvasProps["performanceProfile"];
  nodeCount: number;
  linkCount: number;
  mode: GraphMode;
}

/** Pick a perf tier from the live node + link count (or the host's
 *  explicit override) and turn it into a partial prop bag pre-filling
 *  perf knobs (cooldownTicks, ngraph layout in 3D, lower mesh
 *  resolutions, etc). The bag is spread *before* `props` so anything
 *  the host sets explicitly always wins. */
export function usePerfTierDefaults<
  N extends NodeObject,
  L extends LinkObject,
>(params: UsePerfTierDefaultsParams): Partial<LoraGraphCanvasProps<N, L>> {
  const { profile = "auto", nodeCount, linkCount, mode } = params;
  return useMemo<Partial<LoraGraphCanvasProps<N, L>>>(() => {
    if (profile === "off") return {};
    const tier =
      profile === "auto"
        ? pickPerfTier({ nodeCount, linkCount })
        : profile;
    return perfTierDefaults<N, L>(tier, mode);
  }, [profile, nodeCount, linkCount, mode]);
}
