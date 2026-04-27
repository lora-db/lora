export type WasmSnapshotSource =
  | string
  | URL
  | Uint8Array
  | ArrayBuffer
  | Blob
  | Response
  | ReadableStream<Uint8Array | ArrayBuffer>;
export type WasmSnapshotSaveFormat = "binary" | "base64" | "blob" | "download";
export type WasmSnapshotSaveOptions =
  | { format: "binary" }
  | { format: "base64" }
  | { format: "blob"; mimeType?: string }
  | { format: "download"; filename?: string; mimeType?: string };
export type WasmSnapshotSaveTarget =
  | WasmSnapshotSaveFormat
  | WasmSnapshotSaveOptions
  | undefined;

const DEFAULT_SNAPSHOT_MIME_TYPE = "application/octet-stream";
const DEFAULT_SNAPSHOT_FILENAME = "lora-snapshot.bin";

export function bytesToUint8Array(bytes: Uint8Array | ArrayBuffer): Uint8Array {
  if (bytes instanceof Uint8Array) {
    return bytes;
  }
  return new Uint8Array(bytes);
}

export function snapshotBytesToBase64(bytes: Uint8Array): string {
  const buffer = (globalThis as {
    Buffer?: { from(bytes: Uint8Array): { toString(encoding: "base64"): string } };
  }).Buffer;
  if (buffer) {
    return buffer.from(bytes).toString("base64");
  }

  const chunkSize = 0x8000;
  let binary = "";
  for (let offset = 0; offset < bytes.byteLength; offset += chunkSize) {
    const chunk = bytes.subarray(offset, offset + chunkSize);
    binary += String.fromCharCode(...chunk);
  }
  return btoa(binary);
}

export function snapshotBytesToBlob(
  bytes: Uint8Array,
  mimeType = DEFAULT_SNAPSHOT_MIME_TYPE,
): Blob {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return new Blob([copy.buffer], { type: mimeType });
}

export function downloadSnapshotBytes(
  bytes: Uint8Array,
  filename = DEFAULT_SNAPSHOT_FILENAME,
  mimeType = DEFAULT_SNAPSHOT_MIME_TYPE,
): void {
  const doc = (globalThis as {
    document?: {
      body?: { appendChild(node: unknown): void };
      createElement(tag: "a"): {
        href: string;
        download: string;
        click(): void;
        remove?(): void;
        style?: { display?: string };
      };
    };
  }).document;
  const urlApi = (globalThis as {
    URL?: {
      createObjectURL(blob: Blob): string;
      revokeObjectURL(url: string): void;
    };
  }).URL;

  if (!doc || !urlApi?.createObjectURL) {
    throw new Error("LORA_ERROR: snapshot download requires a browser document");
  }

  const url = urlApi.createObjectURL(snapshotBytesToBlob(bytes, mimeType));
  const anchor = doc.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  if (anchor.style) {
    anchor.style.display = "none";
  }
  doc.body?.appendChild(anchor);
  try {
    anchor.click();
  } finally {
    anchor.remove?.();
    setTimeout(() => urlApi.revokeObjectURL(url), 0);
  }
}

export function resolveSnapshotSaveFormat(
  target: WasmSnapshotSaveTarget,
): WasmSnapshotSaveOptions {
  if (target === undefined) {
    return { format: "binary" };
  }
  if (typeof target === "string") {
    return { format: target };
  }
  return target;
}

export async function snapshotSourceToBytes(
  source: WasmSnapshotSource,
): Promise<Uint8Array> {
  if (source instanceof Uint8Array || source instanceof ArrayBuffer) {
    return bytesToUint8Array(source);
  }

  if (typeof Blob !== "undefined" && source instanceof Blob) {
    return new Uint8Array(await source.arrayBuffer());
  }

  if (typeof Response !== "undefined" && source instanceof Response) {
    return responseToBytes(source);
  }

  if (isReadableStream(source)) {
    return readableStreamToBytes(source);
  }

  return responseToBytes(await fetch(source as string | URL));
}

function isReadableStream(
  source: WasmSnapshotSource,
): source is ReadableStream<Uint8Array | ArrayBuffer> {
  return typeof (source as { getReader?: unknown }).getReader === "function";
}

async function readableStreamToBytes(
  stream: ReadableStream<Uint8Array | ArrayBuffer>,
): Promise<Uint8Array> {
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) {
        const merged = new Uint8Array(total);
        let offset = 0;
        for (const chunk of chunks) {
          merged.set(chunk, offset);
          offset += chunk.byteLength;
        }
        return merged;
      }

      const chunk = bytesToUint8Array(value);
      chunks.push(chunk);
      total += chunk.byteLength;
    }
  } finally {
    reader.releaseLock();
  }
}

async function responseToBytes(response: Response): Promise<Uint8Array> {
  if (!response.ok) {
    throw new Error(
      `LORA_ERROR: snapshot fetch failed (${response.status} ${response.statusText})`,
    );
  }
  return new Uint8Array(await response.arrayBuffer());
}
