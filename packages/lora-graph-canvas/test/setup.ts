// jsdom doesn't implement ResizeObserver or canvas drawing surfaces; the
// kapsule engines call into both. We stub the bits we hit so tests can
// mount the component without exploding.

class MockResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

if (typeof globalThis.ResizeObserver === "undefined") {
  (
    globalThis as unknown as { ResizeObserver: typeof MockResizeObserver }
  ).ResizeObserver = MockResizeObserver;
}

// HTMLCanvasElement.getContext('2d') in jsdom returns null, which would
// crash the engine on mount. The engines are mocked at the import level
// in tests that need them; this stub keeps any leftover paths quiet.
if (typeof HTMLCanvasElement !== "undefined") {
  const proto = HTMLCanvasElement.prototype as unknown as {
    getContext?: () => unknown;
  };
  if (!proto.getContext) {
    proto.getContext = () => ({
      canvas: {} as HTMLCanvasElement,
      clearRect: () => {},
      fillRect: () => {},
      beginPath: () => {},
      moveTo: () => {},
      lineTo: () => {},
      stroke: () => {},
      fill: () => {},
      arc: () => {},
      save: () => {},
      restore: () => {},
      translate: () => {},
      scale: () => {},
      rotate: () => {},
      measureText: () => ({ width: 0 }),
      fillText: () => {},
      setTransform: () => {},
    });
  }
}
