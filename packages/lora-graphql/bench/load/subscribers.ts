// Subscribers for `--subscribers N`: N server-sent-event streams, each
// following one festival by key, held open by the runner while the load
// steps write. The `touch` operation stamps the time it was sent into the
// festival's name, so every event received tells how long the write took
// to reach a subscriber: the request, the commit and the fan-out.

import { get, type IncomingMessage } from "node:http";
import { HOT_FESTIVALS } from "./scenarios.js";

export interface Delivery {
  /** When the write that caused the event was sent (ms since the epoch). */
  sent: number;
  /** How long after that the event arrived, in ms. */
  latency: number;
}

export interface Subscribers {
  count: number;
  /** Events received since the last call. */
  drain(): Delivery[];
  close(): void;
}

/**
 * Open `count` streams across `ports`, subscriber `i` following festival
 * `f(i mod HOT_FESTIVALS)`: a write to a hot festival reaches
 * `count / HOT_FESTIVALS` subscribers. Resolves once every stream is open.
 */
export async function subscribe(
  ports: number[],
  count: number,
  festivals: number,
): Promise<Subscribers> {
  let received: Delivery[] = [];
  const streams: IncomingMessage[] = [];
  const hot = Math.min(HOT_FESTIVALS, festivals);
  const open = (i: number) =>
    new Promise<void>((resolve, reject) => {
      const port = ports[i % ports.length]!;
      const request = get(
        `http://127.0.0.1:${port}/subscribe?key=f${i % hot}`,
        { agent: false },
        (res) => {
          if (res.statusCode !== 200) {
            reject(new Error(`subscribe: HTTP ${res.statusCode}`));
            return;
          }
          streams.push(res);
          res.setEncoding("utf8");
          let buffered = "";
          res.on("data", (chunk: string) => {
            buffered += chunk;
            let end: number;
            while ((end = buffered.indexOf("\n\n")) >= 0) {
              const event = buffered.slice(0, end);
              buffered = buffered.slice(end + 2);
              // The server's first event says the subscription is live.
              if (event === "event: ready") resolve();
              const stamp = /"name":"at (\d+)"/.exec(event);
              if (stamp) {
                const sent = Number(stamp[1]);
                received.push({ sent, latency: Date.now() - sent });
              }
            }
          });
          res.on("error", () => undefined);
        },
      );
      request.on("error", reject);
    });
  // In batches: a burst of thousands of connects overruns the listen backlog.
  for (let i = 0; i < count; i += 200) {
    await Promise.all(
      Array.from({ length: Math.min(200, count - i) }, (_, j) => open(i + j)),
    );
  }
  return {
    count,
    drain: () => {
      const out = received;
      received = [];
      return out;
    },
    close: () => {
      for (const stream of streams) stream.destroy();
    },
  };
}
