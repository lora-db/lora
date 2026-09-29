// S2: plans are checked, not hoped for. Every compiled read carries the
// access path it was written for; `explain()` shows the one the engine
// chose, and a disagreement is a finding.

import type { LoraDriver, PlanNode, QueryPlan, Statement } from "../driver.js";
import type { CompiledRead, SeekExpectation } from "../compile/read.js";

export interface PlanFinding {
  rule:
    | "full-scan"
    | "mutating-read"
    | "result-columns"
    | "scan-expand"
    | "row-budget";
  message: string;
  statement: string;
}

export interface PlanReport {
  statement: Statement;
  plan: QueryPlan;
  /** Operator names from root to leaves, depth first. */
  operators: string[];
  /**
   * The engine's largest row estimate among the statement's scans and
   * seeks (from graph statistics), or null when it has none. A scan that
   * streams in index order under a LIMIT reads fewer rows than this.
   */
  estimatedRows: number | null;
  findings: PlanFinding[];
}

const SEEK_OPERATORS: Record<SeekExpectation["access"], string[]> = {
  exact: ["NodeByPropertyScan"],
  range: ["NodeByPropertyRangeScan", "NodeByPropertyScan"],
  text: ["NodeByTextScan"],
  point: ["NodeByPointScan"],
};

export async function checkPlans(
  driver: LoraDriver,
  compiled: CompiledRead,
  options: { rowBudget?: number } = {},
): Promise<PlanReport[]> {
  if (!driver.explain) {
    throw new Error(
      "the driver has no explain(); plan checks need @loradb/lora-node",
    );
  }
  const reports: PlanReport[] = [];
  for (const [i, statement] of compiled.statements.entries()) {
    const plan = await driver.explain(statement);
    const nodes = flatten(plan.tree);
    const findings: PlanFinding[] = [];
    if (plan.shape !== "readOnly") {
      findings.push({
        rule: "mutating-read",
        message: "a read statement has a mutating plan",
        statement: statement.text,
      });
    }
    if (i === 0 && !sameColumns(plan.resultColumns, compiled.columns)) {
      findings.push({
        rule: "result-columns",
        message: `expected columns [${compiled.columns.join(", ")}], plan returns [${plan.resultColumns.join(", ")}]`,
        statement: statement.text,
      });
    }
    for (const e of compiled.expectations.filter((x) => x.statement === i)) {
      const scans = nodes.filter(
        (n) =>
          n.operator === "NodeByLabelScan" &&
          n.details["labels"]?.split(/[,:|&\s]+/).includes(e.label),
      );
      const seeks = nodes.filter(
        (n) =>
          SEEK_OPERATORS[e.access].includes(n.operator) &&
          n.details["labels"]?.split(/[,:|&\s]+/).includes(e.label),
      );
      if (scans.length > 0 || seeks.length === 0) {
        findings.push({
          rule: "full-scan",
          message: `expected an index ${e.access} seek on :${e.label} (${e.reason}), plan uses ${
            scans.length > 0 ? "NodeByLabelScan" : "no seek"
          }`,
          statement: statement.text,
        });
      }
    }
    const estimates = nodes
      .filter((n) => n.operator.endsWith("Scan") && n.estimatedRows !== null)
      .map((n) => n.estimatedRows!);
    const estimatedRows = estimates.length > 0 ? Math.max(...estimates) : null;
    if (
      options.rowBudget !== undefined &&
      estimatedRows !== null &&
      estimatedRows > options.rowBudget
    ) {
      findings.push({
        rule: "row-budget",
        message: `the engine estimates ${estimatedRows} rows scanned; the budget is ${options.rowBudget}`,
        statement: statement.text,
      });
    }
    reports.push({
      statement,
      plan,
      operators: nodes.map((n) => n.operator),
      estimatedRows,
      findings,
    });
  }
  return reports;
}

const SEEKS = new Set(Object.values(SEEK_OPERATORS).flat());
const BIND_NOTHING = new Set(["Argument", "Unwind", "Filter", "Projection"]);

/**
 * Expansions that start from a full scan: an `Expand` whose source is a
 * `NodeByLabelScan`, or a `NodeScan` with nothing below it that binds the
 * node. A key test in the same pattern as the expansion plans this way,
 * and the statement then walks every relationship of the type. Statements
 * that start from keys (every mutation statement) must have none.
 */
export function scanExpands(
  statement: Statement,
  plan: QueryPlan,
): PlanFinding[] {
  return flatten(plan.tree)
    .filter((n) => n.operator === "Expand")
    .flatMap((expand) => {
      const source = sourceScan(expand);
      return source
        ? [
            {
              rule: "scan-expand" as const,
              message: `Expand ${expand.details["types"] ?? ""} starts from ${source.operator}${
                source.details["labels"] ? ` :${source.details["labels"]}` : ""
              } instead of a seek`,
              statement: statement.text,
            },
          ]
        : [];
    });
}

/** The full scan feeding `expand`, if its source is one. */
function sourceScan(expand: PlanNode): PlanNode | undefined {
  for (let n = expand.children[0]; n; n = n.children[0]) {
    if (SEEKS.has(n.operator)) return undefined;
    if (n.operator === "NodeByLabelScan") return n;
    // A NodeScan over operators that bind no nodes scans every node;
    // over anything else it re-reads a node bound earlier.
    if (
      n.operator === "NodeScan" &&
      flatten(n)
        .slice(1)
        .every((m) => BIND_NOTHING.has(m.operator))
    ) {
      return n;
    }
  }
  return undefined;
}

function flatten(node: PlanNode): PlanNode[] {
  return [node, ...node.children.flatMap(flatten)];
}

function sameColumns(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}
