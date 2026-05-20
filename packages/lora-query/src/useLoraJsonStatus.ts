import { useMemo, useState } from "react";
import type { Diagnostic } from "@codemirror/lint";
import type { LoraJsonEditorProps } from "./LoraJsonEditor";
import type { PathSegment } from "./json/path";
import { formatJsonPath } from "./json/path";

export interface LoraJsonStatus {
  /** Diagnostics emitted by `jsonParseLinter()` + key-constraint linter. */
  diagnostics: Diagnostic[];
  /** True when there are no diagnostics. */
  ok: boolean;
  /** JSON path of the current cursor location (e.g. `["users", 2, "name"]`). */
  cursorPath: PathSegment[];
  /** JSONPath-style string render of {@link cursorPath} (e.g. `$.users[2].name`). */
  cursorPathString: string;
}

const EMPTY: LoraJsonStatus = {
  diagnostics: [],
  ok: true,
  cursorPath: [],
  cursorPathString: "$",
};

/**
 * Convenience hook bundling the JSON editor's diagnostic + cursor
 * callbacks into a single state object the host can render.
 *
 * ```tsx
 * const [status, statusProps] = useLoraJsonStatus();
 * return (
 *   <>
 *     <LoraJsonEditor value={v} onChange={setV} {...statusProps} />
 *     <p>{status.ok ? "Valid JSON" : `${status.diagnostics.length} error(s)`}</p>
 *     <p>{status.cursorPathString}</p>
 *   </>
 * );
 * ```
 */
export function useLoraJsonStatus(): [
  LoraJsonStatus,
  Pick<LoraJsonEditorProps, "onDiagnostics" | "onCursorPath">,
] {
  const [status, setStatus] = useState<LoraJsonStatus>(EMPTY);

  const props = useMemo(
    () => ({
      onDiagnostics: (diagnostics: Diagnostic[]) =>
        setStatus((s) => ({
          ...s,
          diagnostics,
          ok: diagnostics.length === 0,
        })),
      onCursorPath: (path: PathSegment[]) =>
        setStatus((s) => ({
          ...s,
          cursorPath: path,
          cursorPathString: formatJsonPath(path),
        })),
    }),
    [],
  );

  return [status, props];
}
