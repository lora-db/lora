export type WasmSnapshotSource =
  | URL
  | Uint8Array
  | ArrayBuffer
  | Blob
  | Response
  | ReadableStream<Uint8Array | ArrayBuffer>;

export type WasmSnapshotCompression =
  | "none"
  | "gzip"
  | { format: "none" }
  | { format: "gzip"; level?: number };

export interface WasmSnapshotPasswordParams {
  memoryCostKib?: number;
  timeCost?: number;
  parallelism?: number;
}

export type WasmSnapshotEncryption =
  | {
      type?: "password" | "passphrase";
      keyId?: string;
      password: string;
      params?: WasmSnapshotPasswordParams;
    }
  | {
      type: "key" | "rawKey" | "raw_key";
      keyId?: string;
      key: number[];
    };

export interface WasmSnapshotByteOptions {
  compression?: WasmSnapshotCompression;
  encryption?: WasmSnapshotEncryption | null;
}

export interface WasmSnapshotLoadOptions {
  credentials?: WasmSnapshotEncryption | null;
  encryption?: WasmSnapshotEncryption | null;
}

export type WasmSnapshotSaveFormat =
  | "bytes"
  | "arrayBuffer"
  | "blob"
  | "response"
  | "stream"
  | "url";

export type WasmSnapshotSaveOptions =
  | ({ format?: "bytes" } & WasmSnapshotByteOptions)
  | ({ format: "arrayBuffer" } & WasmSnapshotByteOptions)
  | ({ format: "blob"; mimeType?: string } & WasmSnapshotByteOptions)
  | ({ format: "response"; mimeType?: string } & WasmSnapshotByteOptions)
  | ({ format: "stream" } & WasmSnapshotByteOptions)
  | ({ format: "url"; mimeType?: string } & WasmSnapshotByteOptions);

const DEFAULT_SNAPSHOT_MIME_TYPE = "application/octet-stream";

export function asUint8Array(bytes: Uint8Array | ArrayBuffer): Uint8Array {
  if (bytes instanceof Uint8Array) {
    return bytes;
  }
  return new Uint8Array(bytes);
}

export function snapshotAsArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return copy.buffer;
}

export function snapshotAsBlob(
  bytes: Uint8Array,
  mimeType = DEFAULT_SNAPSHOT_MIME_TYPE,
): Blob {
  return new Blob([snapshotAsArrayBuffer(bytes)], { type: mimeType });
}

export function snapshotAsResponse(
  bytes: Uint8Array,
  mimeType = DEFAULT_SNAPSHOT_MIME_TYPE,
): Response {
  return new Response(snapshotAsArrayBuffer(bytes), {
    headers: { "content-type": mimeType },
  });
}

export function snapshotAsReadableStream(
  bytes: Uint8Array,
): ReadableStream<Uint8Array> {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(copy);
      controller.close();
    },
  });
}

export function snapshotAsObjectUrl(
  bytes: Uint8Array,
  mimeType = DEFAULT_SNAPSHOT_MIME_TYPE,
): URL {
  if (typeof URL === "undefined" || typeof URL.createObjectURL !== "function") {
    throw new Error("LORA_ERROR: snapshot URL output requires URL.createObjectURL");
  }
  return new URL(URL.createObjectURL(snapshotAsBlob(bytes, mimeType)));
}

export async function readSnapshotSource(
  source: WasmSnapshotSource,
): Promise<Uint8Array> {
  if (source instanceof Uint8Array || source instanceof ArrayBuffer) {
    return asUint8Array(source);
  }

  if (typeof Blob !== "undefined" && source instanceof Blob) {
    return new Uint8Array(await source.arrayBuffer());
  }

  if (typeof Response !== "undefined" && source instanceof Response) {
    return readSnapshotResponse(source);
  }

  if (source instanceof URL) {
    return readSnapshotResponse(await fetch(source));
  }

  if (isReadableStream(source)) {
    return readSnapshotStream(source);
  }

  throw new Error("LORA_ERROR: unsupported snapshot source");
}

function isReadableStream(
  source: WasmSnapshotSource,
): source is ReadableStream<Uint8Array | ArrayBuffer> {
  return typeof (source as { getReader?: unknown }).getReader === "function";
}

async function readSnapshotStream(
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

      const chunk = asUint8Array(value);
      chunks.push(chunk);
      total += chunk.byteLength;
    }
  } finally {
    reader.releaseLock();
  }
}

async function readSnapshotResponse(response: Response): Promise<Uint8Array> {
  if (!response.ok) {
    throw new Error(
      `LORA_ERROR: snapshot fetch failed (${response.status} ${response.statusText})`,
    );
  }
  return new Uint8Array(await response.arrayBuffer());
}
