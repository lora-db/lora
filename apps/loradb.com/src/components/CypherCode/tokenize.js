// Lightweight Cypher tokenizer for inline snippets.
//
// This is not a full parser. It's a deterministic pattern-matcher tuned to
// the kinds of short strings that appear inline in prose:
//
//   - function calls:             `date()`, `count(DISTINCT n)`
//   - property / attribute access: `n.name`, `dt.year`
//   - clause keywords:             `MATCH (n:Person)`, `WITH *`
//   - simple expressions:          `n.age > 30`, `date + duration`
//
// It purposely ignores exotic Cypher syntax (subqueries, variable-length,
// rare operators) that rarely appears inline — full fenced code blocks
// render via the Prism `cypher` grammar anyway.
//
// Output is a flat array of `{ type, value }` tokens. The renderer maps
// `type` to a CSS class so colours match the existing Prism theme
// in `src/styles/components/_code.scss`.

const KEYWORDS = new Set([
  'MATCH',
  'OPTIONAL',
  'WHERE',
  'RETURN',
  'WITH',
  'CREATE',
  'MERGE',
  'SET',
  'DELETE',
  'DETACH',
  'REMOVE',
  'UNWIND',
  'ORDER',
  'BY',
  'SKIP',
  'LIMIT',
  'ASC',
  'DESC',
  'AND',
  'OR',
  'NOT',
  'XOR',
  'IN',
  'IS',
  'NULL',
  'TRUE',
  'FALSE',
  'AS',
  'DISTINCT',
  'UNION',
  'ALL',
  'CASE',
  'WHEN',
  'THEN',
  'ELSE',
  'END',
  'EXISTS',
  'ON',
  'STARTS',
  'ENDS',
  'CONTAINS',
  'USING',
  'CALL',
  'YIELD',
  'FOREACH',
]);

const RE_STRING_SINGLE = /^'(?:[^'\\]|\\.|'')*'/;
const RE_STRING_DOUBLE = /^"(?:[^"\\]|\\.|"")*"/;
const RE_NUMBER = /^(?:0x[0-9a-fA-F]+|0o[0-7]+|\d+\.\d+(?:e[-+]?\d+)?|\d+(?:e[-+]?\d+)?)/;
const RE_PARAM = /^\$[A-Za-z_][A-Za-z0-9_]*/;
const RE_IDENT = /^[A-Za-z_][A-Za-z0-9_]*/;
// Arrows / range markers — must be tried before single-char operators.
const RE_ARROW = /^(?:<-\[|\]->|<->|->|<-|\*\.\.\d*|\.\.)/;
const RE_OP = /^(?:<>|<=|>=|=~|\+=|-=|\*=|\/=|%=|\^=|\|\||&&|[+\-*/%^|=<>!])/;
const RE_PUNCT = /^[()[\]{},;:|]/;
const RE_WHITESPACE = /^\s+/;

export function tokenizeCypher(input) {
  const out = [];
  let i = 0;
  const n = input.length;

  // Track the previous non-whitespace token so we can classify identifiers
  // contextually (e.g. `name` in `n.name` is a property, not a variable).
  let prev = null;

  const pushToken = (tok) => {
    out.push(tok);
    if (tok.type !== 'whitespace') prev = tok;
  };

  while (i < n) {
    const rest = input.slice(i);

    const ws = RE_WHITESPACE.exec(rest);
    if (ws) {
      out.push({ type: 'whitespace', value: ws[0] });
      i += ws[0].length;
      continue;
    }

    // Strings
    let m = RE_STRING_SINGLE.exec(rest) || RE_STRING_DOUBLE.exec(rest);
    if (m) {
      pushToken({ type: 'string', value: m[0] });
      i += m[0].length;
      continue;
    }

    // Numbers
    m = RE_NUMBER.exec(rest);
    if (m) {
      pushToken({ type: 'number', value: m[0] });
      i += m[0].length;
      continue;
    }

    // Parameters ($name)
    m = RE_PARAM.exec(rest);
    if (m) {
      pushToken({ type: 'parameter', value: m[0] });
      i += m[0].length;
      continue;
    }

    // Relationship arrows / range markers
    m = RE_ARROW.exec(rest);
    if (m) {
      pushToken({ type: 'operator', value: m[0] });
      i += m[0].length;
      continue;
    }

    // Colon — label / relationship-type follows
    if (rest[0] === ':') {
      pushToken({ type: 'punctuation', value: ':' });
      i += 1;
      const idm = RE_IDENT.exec(input.slice(i));
      if (idm) {
        pushToken({ type: 'label', value: idm[0] });
        i += idm[0].length;
      }
      continue;
    }

    // Property access — `.name` after an identifier-like thing
    if (rest[0] === '.') {
      // Capture prev BEFORE pushing the dot, so the property check sees the
      // token preceding the `.` rather than the `.` itself.
      const preceding = prev;
      pushToken({ type: 'punctuation', value: '.' });
      i += 1;
      const idm = RE_IDENT.exec(input.slice(i));
      if (idm) {
        const isProperty =
          preceding !== null &&
          (preceding.type === 'variable' ||
            preceding.type === 'property' ||
            preceding.type === 'label' ||
            preceding.value === ')' ||
            preceding.value === ']');
        pushToken({
          type: isProperty ? 'property' : 'variable',
          value: idm[0],
        });
        i += idm[0].length;
      }
      continue;
    }

    // Operators (before single-char punctuation)
    m = RE_OP.exec(rest);
    if (m) {
      pushToken({ type: 'operator', value: m[0] });
      i += m[0].length;
      continue;
    }

    // Punctuation
    m = RE_PUNCT.exec(rest);
    if (m) {
      pushToken({ type: 'punctuation', value: m[0] });
      i += m[0].length;
      continue;
    }

    // Identifiers — keywords / functions / variables
    m = RE_IDENT.exec(rest);
    if (m) {
      const word = m[0];
      const upper = word.toUpperCase();

      if (KEYWORDS.has(upper)) {
        pushToken({ type: 'keyword', value: word });
      } else {
        // Function if followed (optionally through whitespace) by `(`
        let k = i + word.length;
        while (k < n && /\s/.test(input[k])) k += 1;
        if (input[k] === '(') {
          pushToken({ type: 'function', value: word });
        } else {
          pushToken({ type: 'variable', value: word });
        }
      }
      i += word.length;
      continue;
    }

    // Fallback — never loop
    pushToken({ type: 'plain', value: input[i] });
    i += 1;
  }

  return out;
}
