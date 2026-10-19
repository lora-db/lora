// A relationship quantifier whose filter is empty is left out of the
// statement, so a filter emptied by unset variables costs nothing. Written
// as a literal `{}` in the document, though, it can only have meant "has
// any" (or "has none", "has exactly one"), and leaving it out would match
// every row without a word. It is refused, naming the count filter that
// asks for it: by a validation rule, so `persist()` and `buildManifest()`
// fail on it, and again when arguments are read, for servers that validate
// with the standard rules only. An empty `all` holds on every row, which is
// what leaving it out gives, so it stays allowed.

import {
  GraphQLError,
  getNullableType,
  isInputObjectType,
  isListType,
  Kind,
  type GraphQLInputType,
  type ObjectFieldNode,
  type ValidationRule,
  type ValueNode,
} from "graphql";

const QUANTIFIERS = ["some", "all", "none", "single"];

const INSTEAD = new Map([
  ["some", 'ask `count: { gt: 0 }` for "has any"'],
  ["none", 'ask `count: { eq: 0 }` for "has none"'],
  ["single", 'ask `count: { eq: 1 }` for "has exactly one"'],
]);

/**
 * An object value with nothing in it, written out in the document: `{}`,
 * or only such objects (`{ node: {} }`). A variable or a value anywhere
 * makes it non-empty here, even if it turns out null at run time.
 */
function literallyEmpty(value: ValueNode): boolean {
  return (
    value.kind === Kind.OBJECT &&
    value.fields.every((f) => literallyEmpty(f.value))
  );
}

/**
 * The first literal empty `some`, `none` or `single` in an argument value
 * of `type`, as an error. `name` is the field (or argument) that holds
 * `value`.
 */
export function emptyQuantifier(
  value: ValueNode,
  type: GraphQLInputType,
  name: string,
): GraphQLError | undefined {
  const t = getNullableType(type);
  if (isListType(t)) {
    const items = value.kind === Kind.LIST ? value.values : [value];
    for (const item of items) {
      const found = emptyQuantifier(item, t.ofType, name);
      if (found) return found;
    }
    return undefined;
  }
  if (!isInputObjectType(t) || value.kind !== Kind.OBJECT) return undefined;
  const fields = t.getFields();
  // The relationship and connection filter types are the input types
  // that take all four quantifiers.
  const quantified = QUANTIFIERS.every((q) => q in fields);
  for (const field of value.fields) {
    const def = fields[field.name.value];
    if (!def) continue;
    const instead = quantified ? INSTEAD.get(field.name.value) : undefined;
    if (instead !== undefined && literallyEmpty(field.value)) {
      return refusal(name, field, instead);
    }
    const found = emptyQuantifier(field.value, def.type, field.name.value);
    if (found) return found;
  }
  return undefined;
}

function refusal(
  name: string,
  field: ObjectFieldNode,
  instead: string,
): GraphQLError {
  return new GraphQLError(
    `\`${name}: { ${field.name.value}: {} }\` has an empty filter, which is left out and would match every row; ${instead}`,
    { nodes: field },
  );
}

/** Refuses a literal empty `some`, `none` or `single` (`some: {}`). */
export const emptyQuantifierRule: ValidationRule = (context) => ({
  Argument(node) {
    const arg = context.getArgument();
    if (!arg) return;
    const found = emptyQuantifier(node.value, arg.type, node.name.value);
    if (found) context.reportError(found);
  },
});
