// `@fulltext` and `@vector`: the search indexes of a node type and the
// root fields that query them.

import type { ModelProblem } from "../../errors.js";
import type { SearchIndex, ScalarField } from "../types.js";
import { upperFirst, snakeCase } from "./shapes.js";

export const vectorQueryNames = new WeakMap<ScalarField, string>();

export function readSearch(
  typeName: string,
  label: string,
  defaultName: string,
  plural: string | undefined,
  scalars: ScalarField[],
  fulltext: Record<string, unknown> | undefined,
  problems: ModelProblem[],
): SearchIndex[] {
  const at = (message: string) => problems.push({ type: typeName, message });
  const Plural = upperFirst(plural ?? defaultName);
  const out: SearchIndex[] = [];
  type Raw = {
    name?: string;
    fields: string[];
    analyzer: "STANDARD" | "SIMPLE";
    queryName?: string;
  };
  const raws = (fulltext?.["indexes"] as Raw[] | undefined) ?? [];
  raws.forEach((raw, i) => {
    if (i > 0 && !raw.name) {
      at("@fulltext: every index after the first needs a name");
    }
    const name = raw.name ?? `${snakeCase(label)}_search`;
    const fields: ScalarField[] = [];
    for (const f of raw.fields) {
      const field = scalars.find((x) => x.name === f);
      if (!field) at(`@fulltext: ${typeName} has no field ${f}`);
      // A list of strings indexes each of its strings.
      else if (field.type !== "String" && field.type !== "ID") {
        at(`@fulltext: ${f} is not a String or [String] field`);
      } else if (guardedRead(field)) {
        // The index matches on stored values whatever the rules say, so
        // a search would answer "which rows contain this word" for a
        // value the reader may not read (or reads masked).
        at(
          `@fulltext: ${f} has field-level read rules (${guardedRead(field)}); a search over it would reveal the hidden values. Leave it out of the index`,
        );
      } else fields.push(field);
    }
    if (raw.fields.length === 0) at("@fulltext: an index needs fields");
    out.push({
      kind: "fulltext",
      name,
      fields,
      analyzer: raw.analyzer,
      queryName:
        raw.queryName ??
        (i === 0 ? `search${Plural}` : `search${Plural}By${upperFirst(name)}`),
    });
  });
  const vectors = scalars.filter((f) => f.vector);
  for (const f of vectors) {
    out.push({
      kind: "vector",
      name: `${snakeCase(label)}_${snakeCase(f.property)}_vector`,
      field: f,
      dimensions: f.vector!.dimensions,
      similarity: f.vector!.similarity,
      queryName:
        vectorQueryNames.get(f) ??
        (vectors.length === 1
          ? `similar${Plural}`
          : `similar${Plural}By${upperFirst(f.name)}`),
    });
  }
  const names = new Set<string>();
  for (const x of out) {
    if (names.has(x.name)) at(`search index name ${x.name} is used twice`);
    names.add(x.name);
  }
  return out;
}

/** What guards reading `field` per request or row, if anything. */
function guardedRead(field: ScalarField): string | undefined {
  if (field.authorization?.mask?.length) return "@authorization(mask:)";
  if (field.authorization?.validate?.some((r) => r.operations.has("READ"))) {
    return "@authorization(validate:) for READ";
  }
  if (field.authentication?.has("READ")) return "@authentication for READ";
  return undefined;
}
