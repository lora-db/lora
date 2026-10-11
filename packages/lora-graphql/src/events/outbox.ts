// The delivery state of an `@outbox` node: stored properties the library
// owns, on the node itself, so the work is exactly as durable as the data
// and commits or rolls back with the write that made it. None is a field
// of the API.

export const OUTBOX = {
  /** `PENDING`, `CLAIMED`, `DONE` or `FAILED`. */
  state: "_outbox",
  /** Handler calls that failed so far. */
  attempts: "_outboxAttempts",
  /** Not before this time (epoch ms): the backoff after a failure. */
  at: "_outboxAt",
  /** A claim is void past this time (epoch ms): its worker died. */
  lease: "_outboxLease",
  /** The message of the last failure. */
  error: "_outboxError",
} as const;

/** Whether `property` is one of the delivery-state properties. */
export function isOutboxProperty(property: string): boolean {
  return property.startsWith("_outbox");
}
