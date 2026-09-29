// S2: plans are checked, not hoped for. Every compiled read carries the
// access path it was written for; `explain()` shows the one the engine
// chose, and a disagreement is a finding.

import type { LoraDriver, PlanNode, QueryPlan, Statement } from "../driver.js";
import type { CompiledRead, SeekExpectation } from "../compile/read.js";

export interface PlanFinding {
  rule: "full-scan" | "mutating-read" | "result-columns";
  message: string;
  statement: string;
}

export interface PlanReport {
  statement: Statement;
  plan: QueryPlan;
  /** Operator names from root to leaves, depth first. */
  operators: string[];
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
    reports.push({
      statement,
      plan,
      operators: nodes.map((n) => n.operator),
      findings,
    });
  }
  return reports;
}

function flatten(node: PlanNode): PlanNode[] {
  return [node, ...node.children.flatMap(flatten)];
}

function sameColumns(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}
