// The events a subscriber receives, built from a write-set: one per node
// of a type, plus CONNECT / DISCONNECT events when the type offers them.

import { keyOf } from "../compile/read.js";
import type { WriteChange } from "../execute/changes.js";
import type { NodeType } from "../model/types.js";
import type { ChangeEvent } from "../schema/build.js";

/** The write an event came from, for reads shared across its subscribers. */
export const CHANGE = Symbol("change");
/** On a CONNECT / DISCONNECT event: whether its node owns the field. */
export const SELF_OWNS = Symbol("selfOwns");

/** A write's events for one node type, one per node, most specific first. */
export function changeEvents(
  change: WriteChange,
  node: NodeType,
): ChangeEvent[] {
  const out = new Map<string, ChangeEvent>();
  const previous = new Map<string, Record<string, unknown>>();
  for (const b of change.before ?? []) {
    if (b.type === node.name) previous.set(keyOf(b.key), b.properties);
  }
  const make = (
    operation: ChangeEvent["operation"],
    key: unknown,
    extra: Partial<ChangeEvent> = {},
  ): ChangeEvent => {
    const event: ChangeEvent = {
      operation,
      key,
      timestamp: change.timestamp,
      ...extra,
    };
    if (operation === "UPDATE" || operation === "DELETE") {
      event.previous = previous.get(keyOf(key));
    }
    Object.defineProperty(event, CHANGE, { value: change });
    return event;
  };
  const add = (operation: ChangeEvent["operation"], key: unknown) => {
    const id = keyOf(key);
    if (!out.has(id)) out.set(id, make(operation, key));
  };
  for (const e of change.deleted)
    if (e.type === node.name) add("DELETE", e.key);
  for (const e of change.created)
    if (e.type === node.name) add("CREATE", e.key);
  // Updated nodes, and nodes that gained or lost a relationship.
  for (const e of change.entities)
    if (e.type === node.name) add("UPDATE", e.key);
  const events = [...out.values()];
  if (node.subscriptionOptions.relationships) {
    for (const [operation, refs] of [
      ["CONNECT", change.connected],
      ["DISCONNECT", change.disconnected],
    ] as const) {
      for (const r of refs) {
        for (const [self, other] of [
          [r.from, r.to],
          [r.to, r.from],
        ] as const) {
          if (self.type !== node.name) continue;
          const event = make(operation, self.key, {
            relationship: {
              field: r.field,
              type: r.type,
              relatedType: other.type,
              relatedKey: other.key,
            },
          });
          // The declaring field's owner is `from`: whose rules guard it.
          Object.defineProperty(event, SELF_OWNS, { value: self === r.from });
          events.push(event);
        }
      }
    }
  }
  return events;
}
