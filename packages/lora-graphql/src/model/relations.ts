// Relationships to interfaces and unions. A polymorphic field is handled
// as one concrete copy per member type, so filters, projections and
// mutations reuse the single-target machinery; `via` points back to the
// declared field.

import type { AbstractType, GraphModel, RelationshipField } from "./types.js";

const copies = new WeakMap<RelationshipField, RelationshipField[]>();

export function isPolymorphic(
  model: GraphModel,
  rel: RelationshipField,
): boolean {
  return model.abstracts.has(rel.target);
}

/** The field itself, or one concrete copy per member of its target. */
export function memberFields(
  model: GraphModel,
  rel: RelationshipField,
): RelationshipField[] {
  if (!isPolymorphic(model, rel)) return [rel];
  let list = copies.get(rel);
  if (!list) {
    list = rel.members.map((member) => ({
      ...rel,
      target: member,
      members: [member],
      via: rel,
    }));
    copies.set(rel, list);
  }
  return list;
}

/** The declared field a member copy came from (or the field itself). */
export function declared(rel: RelationshipField): RelationshipField {
  return rel.via ?? rel;
}

export function abstractOf(
  model: GraphModel,
  rel: RelationshipField,
): AbstractType | undefined {
  return model.abstracts.get(rel.target);
}
