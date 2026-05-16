import { describe, it, expect } from "vitest";
import { pickPerfTier, perfTierDefaults } from "../src/utils/perfTier";

describe("pickPerfTier", () => {
  it("returns default below the large threshold", () => {
    expect(pickPerfTier({ nodeCount: 100, linkCount: 200 })).toBe("default");
    expect(pickPerfTier({ nodeCount: 1500, linkCount: 500 })).toBe(
      "default",
    );
  });

  it("returns large between 2k and 10k weighted", () => {
    expect(pickPerfTier({ nodeCount: 2_000, linkCount: 0 })).toBe("large");
    expect(pickPerfTier({ nodeCount: 5_000, linkCount: 5_000 })).toBe(
      "large",
    );
  });

  it("returns xlarge between 10k and 50k weighted", () => {
    expect(pickPerfTier({ nodeCount: 10_000, linkCount: 0 })).toBe("xlarge");
    expect(pickPerfTier({ nodeCount: 20_000, linkCount: 40_000 })).toBe(
      "xlarge",
    );
  });

  it("returns huge at or above 50k weighted", () => {
    expect(pickPerfTier({ nodeCount: 50_000, linkCount: 0 })).toBe("huge");
    expect(pickPerfTier({ nodeCount: 100_000, linkCount: 200_000 })).toBe(
      "huge",
    );
  });

  it("weights links at half a node", () => {
    // 1000 nodes + 2000 links → 2000 weighted → large
    expect(pickPerfTier({ nodeCount: 1_000, linkCount: 2_000 })).toBe(
      "large",
    );
    // 1000 nodes + 1800 links → 1900 weighted → still default
    expect(pickPerfTier({ nodeCount: 1_000, linkCount: 1_800 })).toBe(
      "default",
    );
  });
});

describe("perfTierDefaults", () => {
  it("returns an empty bag for the default tier", () => {
    expect(perfTierDefaults("default", "2d")).toEqual({});
    expect(perfTierDefaults("default", "3d")).toEqual({});
  });

  it("ratchets cooldownTicks down as the tier escalates", () => {
    const large = perfTierDefaults("large", "2d").cooldownTicks!;
    const xlarge = perfTierDefaults("xlarge", "2d").cooldownTicks!;
    const huge = perfTierDefaults("huge", "2d").cooldownTicks!;
    // monotonically decreasing — bigger graph cools off faster.
    expect(large).toBeGreaterThan(xlarge);
    expect(xlarge).toBeGreaterThan(huge);
  });

  it("ratchets d3AlphaDecay up as the tier escalates", () => {
    const large = perfTierDefaults("large", "2d").d3AlphaDecay!;
    const xlarge = perfTierDefaults("xlarge", "2d").d3AlphaDecay!;
    const huge = perfTierDefaults("huge", "2d").d3AlphaDecay!;
    expect(large).toBeLessThan(xlarge);
    expect(xlarge).toBeLessThan(huge);
  });

  it("only injects 3D-specific knobs in 3D mode", () => {
    const twoD = perfTierDefaults("xlarge", "2d");
    expect(twoD.forceEngine).toBeUndefined();
    expect(twoD.nodeResolution).toBeUndefined();
    expect(twoD.linkResolution).toBeUndefined();
    // 2D-specific should be present.
    expect(twoD.autoPauseRedraw).toBe(true);
    expect(twoD.linkLineDash).toBeNull();

    const threeD = perfTierDefaults("xlarge", "3d");
    expect(threeD.forceEngine).toBe("ngraph");
    expect(threeD.nodeResolution).toBeDefined();
    expect(threeD.linkResolution).toBeDefined();
    expect(threeD.nodeOpacity).toBe(1);
    expect(threeD.linkOpacity).toBe(1);
    // 2D-specific should not leak.
    expect(threeD.autoPauseRedraw).toBeUndefined();
  });

  it("drops 3D resolution as the tier escalates", () => {
    const large = perfTierDefaults("large", "3d").nodeResolution!;
    const xlarge = perfTierDefaults("xlarge", "3d").nodeResolution!;
    const huge = perfTierDefaults("huge", "3d").nodeResolution!;
    expect(large).toBeGreaterThanOrEqual(xlarge);
    expect(xlarge).toBeGreaterThanOrEqual(huge);
  });
});
