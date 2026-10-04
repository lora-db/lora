// Document guards. `maxCost` bounds the rows an operation touches, not
// the work of parsing and validating it: these rules bound the document
// itself, before anything is compiled. `execute()` applies them; for any
// other server, `validationRules()` and `envelopPlugin()` do.

import {
  GraphQLError,
  GraphQLSchema,
  Kind,
  ValidationContext,
  type ASTVisitor,
  type FragmentDefinitionNode,
  type ParseOptions,
  type SelectionSetNode,
  type ValidationRule,
} from "graphql";

export interface DocumentGuards {
  /**
   * Deepest field nesting, fragments followed. Default 12. An
   * introspection field (`__schema`, `__type`) counts as one level here:
   * its subtree follows the type-reference chain, not the graph, and is
   * bounded by `maxIntrospectionDepth`.
   */
  maxDepth?: number;
  /**
   * Deepest nesting under `__schema` or `__type`, the field included.
   * Default 20: the standard introspection query (every option on) needs
   * 15, and a runaway `ofType { ofType … }` chain is still refused.
   */
  maxIntrospectionDepth?: number;
  /** Aliased fields in one document. Default 30. */
  maxAliases?: number;
  /** Root fields in one operation. Default 20. */
  maxRootFields?: number;
  /** Lexer tokens in one document, checked while parsing. Default 5000. */
  maxTokens?: number;
  /**
   * Allow `__schema` and `__type`. Default: off when `NODE_ENV` is
   * `production`, on otherwise. `__typename` is always allowed.
   */
  introspection?: boolean;
}

export const DEFAULT_GUARDS = {
  maxDepth: 12,
  maxIntrospectionDepth: 20,
  maxAliases: 30,
  maxRootFields: 20,
  maxTokens: 5000,
} as const;

export function resolveGuards(
  guards: DocumentGuards = {},
): Required<DocumentGuards> {
  return {
    maxDepth: guards.maxDepth ?? DEFAULT_GUARDS.maxDepth,
    maxIntrospectionDepth:
      guards.maxIntrospectionDepth ?? DEFAULT_GUARDS.maxIntrospectionDepth,
    maxAliases: guards.maxAliases ?? DEFAULT_GUARDS.maxAliases,
    maxRootFields: guards.maxRootFields ?? DEFAULT_GUARDS.maxRootFields,
    maxTokens: guards.maxTokens ?? DEFAULT_GUARDS.maxTokens,
    introspection: guards.introspection ?? nodeEnv() !== "production",
  };
}

/** Validation rules for the guards, to add to the standard ones. */
export function validationRules(guards: DocumentGuards = {}): ValidationRule[] {
  const g = resolveGuards(guards);
  const rules: ValidationRule[] = [
    depthRule(g.maxDepth, g.maxIntrospectionDepth),
    aliasRule(g.maxAliases),
    rootFieldRule(g.maxRootFields),
  ];
  if (!g.introspection) rules.push(noIntrospection);
  return rules;
}

/** Parse options for the guards (the token limit). */
export function parseOptions(guards: DocumentGuards = {}): ParseOptions {
  return { maxTokens: resolveGuards(guards).maxTokens };
}

/**
 * The guards as an Envelop plugin, for GraphQL Yoga and other Envelop
 * servers: the token limit wraps `parse`, the rest are validation rules.
 * Typed structurally, so the package does not depend on Envelop.
 *
 * It also checks, once, that the server runs the same `graphql` copy as
 * this library (see `graphqlRealmProblem`), and reports a mismatch
 * through `onRealmMismatch` (default: `console.error`).
 */
export function envelopPlugin(
  guards: DocumentGuards = {},
  onRealmMismatch: (message: string) => void = defaultRealmReport,
) {
  const rules = validationRules(guards);
  const options = parseOptions(guards);
  let reported = false;
  const check = (value: unknown, what: RealmProbe) => {
    if (reported) return;
    const problem = graphqlRealmProblem(value, what);
    if (!problem) return;
    reported = true;
    onRealmMismatch(problem);
  };
  const realmRule: ValidationRule = (context) => {
    check(context, "validation");
    return {};
  };
  return {
    onSchemaChange({ schema }: { schema: unknown }) {
      check(schema, "schema");
    },
    onParse({
      parseFn,
      setParseFn,
    }: {
      parseFn: (source: unknown, options?: ParseOptions) => unknown;
      setParseFn: (
        fn: (source: unknown, options?: ParseOptions) => unknown,
      ) => void;
    }) {
      setParseFn((source, parseOpts) =>
        parseFn(source, { ...parseOpts, ...options }),
      );
    },
    onValidate({
      addValidationRule,
    }: {
      addValidationRule: (rule: ValidationRule) => void;
    }) {
      if (!reported) addValidationRule(realmRule);
      for (const rule of rules) addValidationRule(rule);
    },
  };
}

type RealmProbe = "schema" | "validation";

/**
 * When `value` (a schema, or the context a validation rule receives)
 * comes from a different `graphql` module than the one this library
 * imports, the message explaining it; otherwise undefined. Two copies
 * break `instanceof GraphQLError`, so servers such as Yoga mask the
 * library's errors (FORBIDDEN, BAD_USER_INPUT, ...) as "Unexpected
 * error".
 */
export function graphqlRealmProblem(
  value: unknown,
  what: RealmProbe,
): string | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const expected = what === "schema" ? GraphQLSchema : ValidationContext;
  if (value instanceof expected) return undefined;
  const tag = (value as { [Symbol.toStringTag]?: unknown })[Symbol.toStringTag];
  const name = what === "schema" ? "GraphQLSchema" : "ValidationContext";
  // Not a graphql object at all (a test double, a proxy): nothing to say.
  if (tag !== name && value.constructor?.name !== name) return undefined;
  return (
    `@loradb/lora-graphql: the server's ${name} comes from a different copy of the "graphql" ` +
    "package than the one @loradb/lora-graphql imports. With two copies, errors the library " +
    "raises are not instances of the server's GraphQLError, so they are masked (Yoga reports " +
    '"Unexpected error") and their extensions.code is lost. Fix: install a single graphql ' +
    "version (npm dedupe, or pin it with pnpm.overrides / yarn resolutions / npm overrides) " +
    "and check that `npm ls graphql` shows one copy."
  );
}

function defaultRealmReport(message: string): void {
  console.error(message);
}

const guardError = (message: string) =>
  new GraphQLError(message, { extensions: { code: "LIMIT_EXCEEDED" } });

function fragmentsOf(
  context: ValidationContext,
): Map<string, FragmentDefinitionNode> {
  const out = new Map<string, FragmentDefinitionNode>();
  for (const def of context.getDocument().definitions) {
    if (def.kind === Kind.FRAGMENT_DEFINITION) out.set(def.name.value, def);
  }
  return out;
}

/**
 * Deepest field nesting under `set`. A fragment's depth is measured once
 * (`memo`), so spreading one fragment many times costs nothing extra; a
 * fragment cycle counts as 0 here and is reported by the standard rules.
 */
function depthOf(
  set: SelectionSetNode,
  fragments: Map<string, FragmentDefinitionNode>,
  memo: Map<string, number>,
): number {
  let deepest = 0;
  for (const sel of set.selections) {
    let d = 0;
    if (sel.kind === Kind.FIELD) {
      d =
        sel.selectionSet && !INTROSPECTION_ROOTS.has(sel.name.value)
          ? 1 + depthOf(sel.selectionSet, fragments, memo)
          : 1;
    } else if (sel.kind === Kind.INLINE_FRAGMENT) {
      d = depthOf(sel.selectionSet, fragments, memo);
    } else {
      const name = sel.name.value;
      const known = memo.get(name);
      if (known !== undefined) d = known;
      else {
        const frag = fragments.get(name);
        if (!frag) continue;
        memo.set(name, 0);
        d = depthOf(frag.selectionSet, fragments, memo);
        memo.set(name, d);
      }
    }
    deepest = Math.max(deepest, d);
  }
  return deepest;
}

/** Fields whose subtree is measured by `maxIntrospectionDepth`. */
const INTROSPECTION_ROOTS = new Set(["__schema", "__type"]);

/**
 * Deepest introspection field (`__schema`, `__type`) under `set`, counted
 * from that field. `seen` stops fragment cycles.
 */
function introspectionDepthOf(
  set: SelectionSetNode,
  fragments: Map<string, FragmentDefinitionNode>,
  memo: Map<string, number>,
  seen: Set<string> = new Set(),
): number {
  let deepest = 0;
  for (const sel of set.selections) {
    let d = 0;
    if (sel.kind === Kind.FIELD) {
      if (!sel.selectionSet) continue;
      d = INTROSPECTION_ROOTS.has(sel.name.value)
        ? 1 + depthOf(sel.selectionSet, fragments, memo)
        : introspectionDepthOf(sel.selectionSet, fragments, memo, seen);
    } else if (sel.kind === Kind.INLINE_FRAGMENT) {
      d = introspectionDepthOf(sel.selectionSet, fragments, memo, seen);
    } else {
      const frag = fragments.get(sel.name.value);
      if (!frag || seen.has(sel.name.value)) continue;
      seen.add(sel.name.value);
      d = introspectionDepthOf(frag.selectionSet, fragments, memo, seen);
    }
    deepest = Math.max(deepest, d);
  }
  return deepest;
}

function depthRule(max: number, maxIntrospection: number): ValidationRule {
  return (context) => {
    const fragments = fragmentsOf(context);
    return {
      OperationDefinition(op) {
        const memo = new Map<string, number>();
        const depth = depthOf(op.selectionSet, fragments, memo);
        if (depth > max) {
          context.reportError(
            guardError(`the operation nests fields deeper than ${max} levels`),
          );
        }
        const introspection = introspectionDepthOf(
          op.selectionSet,
          fragments,
          memo,
        );
        if (introspection > maxIntrospection) {
          context.reportError(
            guardError(
              `the operation nests introspection deeper than ${maxIntrospection} levels`,
            ),
          );
        }
      },
    } satisfies ASTVisitor;
  };
}

function aliasRule(max: number): ValidationRule {
  return (context) => {
    let count = 0;
    let reported = false;
    return {
      Field(field) {
        if (!field.alias) return;
        count++;
        if (count > max && !reported) {
          reported = true;
          context.reportError(
            guardError(`the document has more than ${max} aliased fields`),
          );
        }
      },
    } satisfies ASTVisitor;
  };
}

/** Root fields of an operation, through fragments at the root. */
function rootFields(
  set: SelectionSetNode,
  fragments: Map<string, FragmentDefinitionNode>,
  seen: Set<string>,
): number {
  let n = 0;
  for (const sel of set.selections) {
    if (sel.kind === Kind.FIELD) n++;
    else if (sel.kind === Kind.INLINE_FRAGMENT) {
      n += rootFields(sel.selectionSet, fragments, seen);
    } else {
      const frag = fragments.get(sel.name.value);
      if (!frag || seen.has(sel.name.value)) continue;
      seen.add(sel.name.value);
      n += rootFields(frag.selectionSet, fragments, seen);
    }
  }
  return n;
}

function rootFieldRule(max: number): ValidationRule {
  return (context) => {
    const fragments = fragmentsOf(context);
    return {
      OperationDefinition(op) {
        if (rootFields(op.selectionSet, fragments, new Set()) > max) {
          context.reportError(
            guardError(`the operation has more than ${max} root fields`),
          );
        }
      },
    } satisfies ASTVisitor;
  };
}

const noIntrospection: ValidationRule = (context) => ({
  Field(field) {
    const name = field.name.value;
    if (name === "__schema" || name === "__type") {
      context.reportError(
        new GraphQLError("introspection is disabled", {
          nodes: [field],
          extensions: { code: "FORBIDDEN" },
        }),
      );
    }
  },
});

export function nodeEnv(): string | undefined {
  return (
    globalThis as { process?: { env?: Record<string, string | undefined> } }
  ).process?.env?.["NODE_ENV"];
}
