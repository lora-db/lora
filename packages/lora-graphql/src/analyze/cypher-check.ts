// S4, online half: every @cypher statement is planned with `explain()`
// before any request runs it, so a broken statement fails `check()` with
// its location and the engine's message.

import { name } from "../compile/cypher.js";
import type { LoraDriver } from "../driver.js";
import { renameParams } from "../model/cypher-lexer.js";
import type { CypherField, GraphModel } from "../model/types.js";

export interface CypherFinding {
  type: string;
  field: string;
  message: string;
}

export async function checkCypherFields(
  driver: LoraDriver,
  model: GraphModel,
): Promise<CypherFinding[]> {
  if (!driver.explain) return [];
  const findings: CypherFinding[] = [];
  const fields: CypherField[] = [
    ...model.queries,
    ...model.mutations,
    ...[...model.nodes.values()].flatMap((n) =>
      [...n.fields.values()].filter(
        (f): f is CypherField => f.kind === "cypher",
      ),
    ),
  ];
  for (const field of fields) {
    const params: Record<string, unknown> = {};
    const statement = renameParams(field.statement, (p) => {
      params[`a_${p}`] = null;
      return `a_${p}`;
    });
    const owner = model.nodes.get(field.owner);
    const text = owner
      ? `MATCH (this:${name(owner.labels[0]!)}) WITH this LIMIT 1\nCALL {\n  WITH this\n${statement}\n}\nRETURN ${name(field.columnName)} AS value`
      : `CALL {\n${statement}\n}\nRETURN ${name(field.columnName)} AS value`;
    const at = (message: string) =>
      findings.push({ type: field.owner, field: field.name, message });
    try {
      const plan = await driver.explain({ text, params: params as never });
      if (plan.shape === "mutating" && field.owner !== "Mutation") {
        at("the statement writes; only Mutation fields may");
      }
    } catch (err) {
      at(err instanceof Error ? err.message : String(err));
    }
  }
  return findings;
}
