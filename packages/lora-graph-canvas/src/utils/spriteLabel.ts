import {
  CanvasTexture,
  LinearFilter,
  Sprite,
  SpriteMaterial,
  type Texture,
  Vector2,
  Vector3,
  type WebGLRenderer,
} from "three";

// Per-frame shared scratch for the `pixelHeight` sprite scaling. Every
// node + link label sprite runs the same FOV / canvas-height math on
// every frame; the values are identical for every sprite within a
// frame, so we compute them once and reuse them. Keyed by
// `renderer.info.render.frame` (a monotonically increasing per-render
// counter) so a fresh frame invalidates the cache without us having to
// touch state from outside the render loop.
//
// One slot is enough because the kapsule uses a single renderer; if a
// host ever drives multiple renderers, the worst case is one stale
// frame on the second renderer before the cache is overwritten — the
// values it carries (FOV, canvas height) are scene-invariant anyway.
interface FrameScratch {
  frameId: number;
  fov: number;
  tanHalfFov: number;
  canvasHeightPx: number;
}
const frameScratch: FrameScratch = {
  frameId: -1,
  fov: -1,
  tanHalfFov: 0,
  canvasHeightPx: 1,
};
const scratchRendererSize = new Vector2();

function refreshFrameScratch(
  renderer: WebGLRenderer,
  fov: number,
): FrameScratch {
  const frameId = renderer.info?.render?.frame ?? -1;
  if (frameId === frameScratch.frameId && fov === frameScratch.fov) {
    return frameScratch;
  }
  renderer.getSize(scratchRendererSize);
  frameScratch.frameId = frameId;
  frameScratch.fov = fov;
  frameScratch.tanHalfFov = Math.tan(((fov * Math.PI) / 180) / 2);
  frameScratch.canvasHeightPx = scratchRendererSize.y || 1;
  return frameScratch;
}

export interface SpriteLabelOpts {
  text: string;
  /** Font size used when rasterising into the canvas texture. Larger
   *  values → sharper text when the camera is close. Defaults to 32. */
  fontSize?: number;
  fontFamily?: string;
  color?: string;
  /** CSS color string for the background pill. Pass an empty string
   *  for no background. Defaults to a semi-opaque dark pill. */
  backgroundColor?: string;
  /** Optional foreground for the alternate "selected" rasterisation.
   *  When supplied alongside `selectedBackgroundColor`, a second
   *  texture is baked at construction and stored on
   *  `sprite.userData.selectedTexture`. Swap `material.map` between
   *  that and `sprite.userData.normalTexture` to toggle styling
   *  without re-rasterising on each selection change. */
  selectedColor?: string;
  /** Optional pill background for the alternate "selected"
   *  rasterisation. See `selectedColor`. */
  selectedBackgroundColor?: string;
  /** Padding in canvas pixels around the rasterised text. */
  padding?: number;
  /** World-space height of the resulting sprite. Width derives from
   *  the rasterised aspect ratio. Defaults to 4. Ignored when
   *  `pixelHeight` is set. */
  worldHeight?: number;
  /** When provided, the sprite auto-scales each frame so its on-screen
   *  height stays at roughly this many CSS pixels regardless of camera
   *  distance — the constant-screen-size behaviour 2D labels get for
   *  free via `fontSize / globalScale`. Overrides `worldHeight`.
   *  Requires a PerspectiveCamera (the kapsule's default). */
  pixelHeight?: number;
}

/** Extra fields stashed on `sprite.userData` for labels built with
 *  alternate "selected" styling. Consumers can flip
 *  `material.map = userData.selectedTexture` to switch styles without
 *  rebuilding the sprite. */
export interface SpriteLabelUserData {
  normalTexture?: Texture;
  selectedTexture?: Texture;
}

/** Build a billboarded text sprite for use as a 3D label.
 *
 *  Rasterises the text once into a `CanvasTexture` and wraps it in
 *  a `Sprite`. The sprite always faces the camera (that's the
 *  `Sprite` semantic) and respects depth so it occludes nodes
 *  behind it correctly while remaining flat.
 *
 *  Allocates a fresh canvas + texture + material per call. Hosts
 *  rendering 10k+ labels should dispose them when the corresponding
 *  node / link is removed — we don't keep a global cache because the
 *  text content varies per item, and pooling by text+style alone
 *  would let stale sprites pin GPU memory after data updates. */
export function createTextSprite(opts: SpriteLabelOpts): Sprite {
  const {
    text,
    fontSize = 32,
    fontFamily = "system-ui, sans-serif",
    color = "#ffffff",
    backgroundColor = "rgba(28, 31, 35, 0.85)",
    padding = 8,
    worldHeight = 4,
  } = opts;

  // Measure once — the alternate "selected" rasterisation uses the
  // same text + font + padding, so its canvas dimensions match and
  // both textures share the same aspect ratio.
  const probeCanvas = document.createElement("canvas");
  const measureCtx = probeCanvas.getContext("2d");
  if (!measureCtx) {
    // jsdom or a context-less environment — return an empty sprite so
    // the caller doesn't have to null-check.
    return new Sprite(new SpriteMaterial());
  }
  measureCtx.font = `${fontSize}px ${fontFamily}`;
  const textWidth = Math.ceil(measureCtx.measureText(text).width);
  const textHeight = Math.ceil(fontSize * 1.2);
  const cvWidth = textWidth + padding * 2;
  const cvHeight = textHeight + padding * 2;

  const paintCanvas = (
    cv: HTMLCanvasElement,
    fg: string,
    bg: string,
  ): void => {
    cv.width = cvWidth;
    cv.height = cvHeight;
    const ctx = cv.getContext("2d")!;
    ctx.font = `${fontSize}px ${fontFamily}`;
    if (bg) {
      ctx.fillStyle = bg;
      const radius = Math.min(cv.height / 2, 8);
      const anyCtx = ctx as unknown as {
        roundRect?: (
          x: number,
          y: number,
          w: number,
          h: number,
          r: number,
        ) => void;
      };
      ctx.beginPath();
      if (typeof anyCtx.roundRect === "function") {
        anyCtx.roundRect(0, 0, cv.width, cv.height, radius);
      } else {
        ctx.rect(0, 0, cv.width, cv.height);
      }
      ctx.fill();
    }
    ctx.fillStyle = fg;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(text, cv.width / 2, cv.height / 2);
  };

  const normalCanvas = document.createElement("canvas");
  paintCanvas(normalCanvas, color, backgroundColor);
  const texture = new CanvasTexture(normalCanvas);
  texture.minFilter = LinearFilter;
  texture.magFilter = LinearFilter;
  texture.needsUpdate = true;

  // Build the alternate texture only when both colours were supplied.
  // Lets callers opt out (and pay zero extra rasterisation cost) when
  // they don't need a selected-state styling.
  let selectedTexture: CanvasTexture | undefined;
  if (
    opts.selectedColor !== undefined &&
    opts.selectedBackgroundColor !== undefined
  ) {
    const altCanvas = document.createElement("canvas");
    paintCanvas(altCanvas, opts.selectedColor, opts.selectedBackgroundColor);
    selectedTexture = new CanvasTexture(altCanvas);
    selectedTexture.minFilter = LinearFilter;
    selectedTexture.magFilter = LinearFilter;
    selectedTexture.needsUpdate = true;
  }

  const material = new SpriteMaterial({
    map: texture,
    transparent: true,
    depthWrite: false,
  });
  const sprite = new Sprite(material);
  const userData = sprite.userData as SpriteLabelUserData;
  userData.normalTexture = texture;
  if (selectedTexture) userData.selectedTexture = selectedTexture;
  // Preserve the rasterised aspect ratio so wider labels read
  // correctly without distortion.
  const aspect = cvWidth / cvHeight;
  sprite.scale.set(worldHeight * aspect, worldHeight, 1);

  if (opts.pixelHeight !== undefined && opts.pixelHeight > 0) {
    // Constant-screen-size mode. Recompute world height each frame
    // from the perspective-camera projection so the sprite occupies
    // ~`pixelHeight` CSS pixels at any camera distance.
    //
    //   on-screen-px = world-height * (canvasHeight / 2)
    //                                / (distance * tan(fov/2))
    //
    // Solving for world-height gives the formula below. Width is
    // derived from the rasterised aspect so the pill never warps.
    const targetPx = opts.pixelHeight;
    const worldPos = new Vector3();
    // Per-sprite cache of the last applied world height so we can
    // skip the `sprite.scale.set` call when the camera hasn't moved
    // enough to matter (e.g. idle frames between mouse events). The
    // GPU still re-renders the sprite — that's three.js territory —
    // but at least we don't rewrite the scale buffer for nothing.
    let lastWorldH = -1;
    sprite.onBeforeRender = (renderer, _scene, camera) => {
      const cam = camera as unknown as {
        isPerspectiveCamera?: boolean;
        fov?: number;
        position: Vector3;
      };
      if (!cam.isPerspectiveCamera || typeof cam.fov !== "number") return;
      sprite.getWorldPosition(worldPos);
      const distance = cam.position.distanceTo(worldPos);
      // Guard against zero-distance frames (sprite stuck on camera)
      // — would scale to 0 and the sprite would vanish.
      if (distance <= 0) return;
      // Shared per-frame FOV + canvas-height lookup. Cheap when this
      // sprite is the 2nd, 3rd, … sprite in the frame.
      const f = refreshFrameScratch(renderer, cam.fov);
      const worldH = (2 * f.tanHalfFov * distance * targetPx) / f.canvasHeightPx;
      // Sub-pixel change → not worth rewriting. The browser's
      // animation cadence guarantees we revisit soon anyway.
      if (lastWorldH > 0 && Math.abs(worldH - lastWorldH) < lastWorldH * 0.005) {
        return;
      }
      lastWorldH = worldH;
      sprite.scale.set(worldH * aspect, worldH, 1);
    };
  }

  return sprite;
}
