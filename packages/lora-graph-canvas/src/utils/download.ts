/** Trigger a browser download for a given Blob. */
export function downloadBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}

/** Snapshot the supplied canvas to a PNG and trigger a download. */
export function downloadScreenshot(
  canvas: HTMLCanvasElement | null | undefined,
): void {
  if (!canvas) return;
  canvas.toBlob((blob) => {
    if (!blob) return;
    downloadBlob(blob, `lora-graph-${Date.now()}.png`);
  });
}
