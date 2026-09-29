import { requestError } from "../errors.js";

/**
 * Nodes carrying `__authorized: false` failed an `@authorization`
 * validate rule for READ: reading them is an error, not an empty result.
 */
export function assertReadable<T>(value: T): T {
  if (Array.isArray(value)) {
    for (const item of value) assertReadable(item);
  } else if (
    value !== null &&
    typeof value === "object" &&
    (value as { __authorized?: unknown }).__authorized === false
  ) {
    throw requestError("FORBIDDEN", "not allowed to read this node");
  }
  return value;
}
