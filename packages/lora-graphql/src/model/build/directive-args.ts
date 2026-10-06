// Reading a directive's arguments off a definition and its extensions,
// reporting what graphql-js would let through silently: a directive
// written twice, an input field its types do not define.

import {
  getDirectiveValues,
  isInputObjectType,
  isListType,
  isNonNullType,
  Kind,
  type ConstDirectiveNode,
  type ConstValueNode,
  type GraphQLDirective,
  type GraphQLInputType,
} from "graphql";

/**
 * The arguments of `def` on `node` (a type, field, argument or the schema),
 * undefined when it is absent. A type's directives are read from its
 * definition and every extension (`extend type T @authorization(...)`),
 * so a directive on an extension is never silently dropped; a
 * non-repeatable one written more than once across them is a problem
 * naming each place. Repeatable directives are read with `directiveNodes`.
 */
/** Input-object fields in a directive argument that its type lacks. */
function unknownInputFields(
  type: GraphQLInputType,
  value: ConstValueNode,
  path: string,
): Array<{ path: string; name: string; known: string[] }> {
  const named = isNonNullType(type) ? type.ofType : type;
  if (isListType(named)) {
    return value.kind === Kind.LIST
      ? value.values.flatMap((v, i) =>
          unknownInputFields(named.ofType, v, `${path}[${i}]`),
        )
      : unknownInputFields(named.ofType, value, path);
  }
  if (!isInputObjectType(named) || value.kind !== Kind.OBJECT) return [];
  const fields = named.getFields();
  return value.fields.flatMap((f) => {
    const field = fields[f.name.value];
    return field
      ? unknownInputFields(field.type, f.value, `${path}.${f.name.value}`)
      : [{ path, name: f.name.value, known: Object.keys(fields) }];
  });
}

export function directive(
  def: GraphQLDirective,
  node: { astNode?: unknown; extensionASTNodes?: unknown },
  at: (message: string) => void,
): Record<string, unknown> | undefined {
  const found = directiveNodes(def.name, node);
  if (found.length === 0) return undefined;
  if (found.length > 1 && !def.isRepeatable) {
    at(
      `@${def.name} is written ${found.length} times (${found.map((x) => x.place).join(", ")}); write it once`,
    );
  }
  // graphql-js 16 drops input fields a directive's types do not define
  // (17 refuses them): a misspelt `operations` would silently fall back
  // to the default.
  let unknown = false;
  for (const arg of found[0]!.node.arguments ?? []) {
    const type = def.args.find((a) => a.name === arg.name.value)?.type;
    if (!type) continue;
    for (const { path, name, known } of unknownInputFields(
      type,
      arg.value,
      arg.name.value,
    )) {
      unknown = true;
      at(
        `@${def.name}: ${path} has no field ${name} (expected ${known.join(", ")})`,
      );
    }
  }
  if (unknown) return undefined;
  try {
    return getDirectiveValues(def, { directives: [found[0]!.node] });
  } catch (err) {
    at(`@${def.name}: ${err instanceof Error ? err.message : String(err)}`);
    return undefined;
  }
}

/**
 * Every use of the directive `name` on `node`: its definition's, then each
 * extension's, with where it was written (for messages).
 */
export function directiveNodes(
  name: string,
  node: { astNode?: unknown; extensionASTNodes?: unknown },
): Array<{ node: ConstDirectiveNode; place: string }> {
  type WithDirectives = {
    directives?: readonly ConstDirectiveNode[];
    loc?: { startToken: { line: number } };
  } | null;
  const out: Array<{ node: ConstDirectiveNode; place: string }> = [];
  const visit = (ast: WithDirectives | undefined, kind: string) => {
    for (const dir of ast?.directives ?? []) {
      if (dir.name.value !== name) continue;
      const line = dir.loc?.startToken.line;
      out.push({
        node: dir,
        place: line !== undefined ? `${kind}, line ${line}` : kind,
      });
    }
  };
  visit(node.astNode as WithDirectives, "the definition");
  for (const ext of (node.extensionASTNodes as WithDirectives[] | undefined) ??
    []) {
    visit(ext, "an extension");
  }
  return out;
}
