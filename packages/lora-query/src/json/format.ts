/**
 * JSON prettifier + minifier. Built on the native `JSON.parse` +
 * `JSON.stringify` round-trip — no extra dependency.
 *
 * Both helpers follow the same rule as the Cypher `format()` in
 * `parser.ts`: on a parse failure, return the original source
 * unchanged so the editor never destroys partial work mid-edit.
 */

/**
 * Reformat a JSON source string with the requested indent (default
 * 2 spaces). Returns the input unchanged when it does not parse.
 */
export function formatJson(source: string, indent = 2): string {
  try {
    return JSON.stringify(JSON.parse(source), null, indent);
  } catch {
    return source;
  }
}

/**
 * Minify a JSON source string — strips whitespace, keeps the
 * structure. Returns the input unchanged when it does not parse.
 */
export function minifyJson(source: string): string {
  try {
    return JSON.stringify(JSON.parse(source));
  } catch {
    return source;
  }
}
