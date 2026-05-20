import { useMemo, useState } from "react";
import type { Analysis, Outline, OutlineVariable, ParseError } from "./parser";
import type { LoraQueryEditorProps } from "./LoraQueryEditor";

export interface LoraQueryStatus {
  /** Syntax errors emitted by the parser. */
  errors: ParseError[];
  /** Semantic warnings + info diagnostics (undeclared vars, schema mismatches, ...). */
  warnings: ParseError[];
  /** `$param` names referenced anywhere in the query. */
  parameters: string[];
  /** Variables declared anywhere in the query. */
  variables: OutlineVariable[];
  /** Distinct labels observed across all node patterns. */
  labels: string[];
  /** Distinct relationship types observed across all rel patterns. */
  relTypes: string[];
}

const EMPTY: LoraQueryStatus = {
  errors: [],
  warnings: [],
  parameters: [],
  variables: [],
  labels: [],
  relTypes: [],
};

/**
 * Convenience hook that exposes everything the editor knows about the
 * current document — syntax errors, semantic warnings, declared
 * variables, parameter references, and distinct labels / rel types.
 *
 * ```tsx
 * const [status, statusProps] = useLoraQueryStatus();
 * return (
 *   <>
 *     <LoraQueryEditor value={v} onChange={setV} {...statusProps} />
 *     <p>{status.errors.length} errors, {status.warnings.length} warnings</p>
 *   </>
 * );
 * ```
 */
export function useLoraQueryStatus(): [
  LoraQueryStatus,
  Pick<LoraQueryEditorProps, "onDiagnostics" | "onAnalysis" | "onOutline">,
] {
  const [status, setStatus] = useState<LoraQueryStatus>(EMPTY);

  const props = useMemo(
    () => ({
      onDiagnostics: (errors: ParseError[]) =>
        setStatus((s) => ({ ...s, errors })),
      onAnalysis: (a: Analysis) =>
        setStatus((s) => ({ ...s, warnings: a.diagnostics })),
      onOutline: (o: Outline) =>
        setStatus((s) => ({
          ...s,
          parameters: o.parameters,
          variables: o.variables,
          labels: o.labels,
          relTypes: o.relTypes,
        })),
    }),
    [],
  );

  return [status, props];
}
