import React from "react";

/**
 * Returns the prettified form of a Cypher source string.
 *
 * The formatter lives in `@loradb/lora-query` and is WASM-backed, so we
 * dynamic-import it on first call to keep the WASM out of the main
 * bundle and out of SSR (where it would not load). The returned value
 * is the raw `source` until the module resolves, then snaps to the
 * formatted version — `formatSync` is a pure pass-through when the
 * input does not parse, so this never destroys partial work.
 *
 * Shared by `LoraQueryCodeBlock` and any other site that renders a
 * Cypher snippet outside the fenced-code-block path (eg. the
 * `CYPHER_COVERAGE` cards on `/features`). Using one hook keeps the
 * "all queries auto-format on load" contract in a single place.
 */
export function useFormattedCypher(source) {
  const [formatted, setFormatted] = React.useState(source);

  React.useEffect(() => {
    let cancelled = false;
    import("@loradb/lora-query")
      .then(async (mod) => {
        if (mod.__tla) await mod.__tla;
        if (cancelled) return;
        if (typeof mod.formatSync !== "function") return;
        try {
          const pretty = mod.formatSync(source);
          if (!cancelled && pretty !== source) {
            setFormatted(pretty);
          }
        } catch {
          /* keep raw source on formatter error */
        }
      })
      .catch(() => {
        /* keep raw source on import failure */
      });
    return () => {
      cancelled = true;
    };
  }, [source]);

  return formatted;
}
