import { linter, type Diagnostic } from "@codemirror/lint";
import { analyseAll, validateAll, type ParseError } from "../parser";
import { getProviders } from "./providers";

/**
 * CodeMirror linter combining the WASM parser's syntax check with a
 * second-pass semantic analysis. Each diagnostic is rendered with its
 * structured summary, friendly suggestions, and (for syntax errors)
 * pest's full positional report.
 */
export const cypherLinter = linter(
  async (view): Promise<Diagnostic[]> => {
    const source = view.state.doc.toString();
    if (!source.trim()) return [];

    const providers = getProviders(view.state);

    let syntactic: ParseError[] = [];
    try {
      // `validateAll` splits the doc at top-level `;` and runs the
      // parser per statement, translating spans back to the original
      // source. This is what makes multi-statement scripts surface
      // per-statement diagnostics instead of one error at the first
      // `;`.
      syntactic = await validateAll(source);
    } catch (err) {
      return [
        {
          from: 0,
          to: source.length,
          severity: "error",
          message: `Parser crashed: ${(err as Error).message ?? String(err)}`,
        },
      ];
    }

    let semantic: ParseError[] = [];
    // Run the semantic pass even when *some* statements have syntax
    // errors — `analyseAll` iterates per top-level statement and the
    // WASM `analyse` returns an empty diagnostic list for slices that
    // don't parse, so we don't pile semantic noise on top of a
    // syntactic error. The all-or-nothing gate used to be here meant a
    // broken statement disabled the RETURN / undeclared-variable
    // checks on every *other* clean statement in the same script.
    try {
      const a = await analyseAll(source, {
        labels: providers.labels,
        relTypes: providers.relTypes,
        // Auto-enable strict mode when the host provided a list.
        strictLabels: providers.labels.length > 0,
        strictRelTypes: providers.relTypes.length > 0,
      });
      semantic = a.diagnostics;
    } catch {
      // ignore — surface the syntactic ones only
    }

    return [...syntactic, ...semantic].map((err) => ({
      from: clamp(err.span.start, 0, source.length),
      to: clamp(Math.max(err.span.end, err.span.start + 1), 0, source.length),
      severity: err.severity,
      message: err.message,
      renderMessage: () => renderRich(err),
    }));
  },
  { delay: 250 },
);

function clamp(n: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, n));
}

function renderRich(err: ParseError): HTMLElement {
  const root = document.createElement("div");
  root.className = `cm-lora-diagnostic cm-lora-diagnostic--${err.severity}`;

  const summary = document.createElement("div");
  summary.className = "cm-lora-diagnostic__summary";
  for (const line of err.message.split("\n")) {
    if (!line) continue;
    const p = document.createElement("div");
    p.textContent = line;
    summary.appendChild(p);
  }
  root.appendChild(summary);

  if (err.examples?.length) {
    const tryHeader = document.createElement("div");
    tryHeader.className = "cm-lora-diagnostic__hint";
    tryHeader.textContent = "Try one of:";
    root.appendChild(tryHeader);

    const ul = document.createElement("ul");
    ul.className = "cm-lora-diagnostic__examples";
    for (const ex of err.examples) {
      const li = document.createElement("li");
      const code = document.createElement("code");
      code.textContent = ex;
      li.appendChild(code);
      ul.appendChild(li);
    }
    root.appendChild(ul);
  }

  if (err.details) {
    const details = document.createElement("pre");
    details.className = "cm-lora-diagnostic__details";
    details.textContent = err.details;
    root.appendChild(details);
  }

  return root;
}
