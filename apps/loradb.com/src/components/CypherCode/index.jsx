import React from 'react';
import clsx from 'clsx';
import { tokenizeCypher } from './tokenize';
import styles from './styles.module.scss';

/**
 * Inline Cypher code reference.
 *
 * Use for short Cypher snippets in prose, tables, or lists where the
 * extra visual weight of a full fenced code block would be overkill.
 *
 * Two equivalent APIs — pick whichever reads better:
 *
 *   <CypherCode code="date()" />
 *   <CypherCode>date()</CypherCode>
 *
 * Children must be a plain string; anything non-string is rendered as-is
 * without tokenization (avoids surprises with nested JSX).
 *
 * Non-Cypher references (filenames, CLI flags, package names) should
 * continue to use plain backticks — see docs/CONTRIBUTING-DOCS.md.
 */
function resolveSource(code, children) {
  if (typeof code === 'string') return code;
  if (typeof children === 'string') return children;
  if (Array.isArray(children)) {
    return children.filter((c) => typeof c === 'string').join('');
  }
  return '';
}

export default function CypherCode({ code, children, className }) {
  const source = resolveSource(code, children);
  const tokens = React.useMemo(() => tokenizeCypher(source), [source]);

  return (
    <code
      className={clsx(styles.cypherCode, 'cypher-inline', className)}
      // Keep copy-paste of the raw source reliable, independent of the
      // tokenized DOM.
      data-cypher-source={source}
    >
      {tokens.map((tok, idx) => {
        if (tok.type === 'whitespace') {
          return <React.Fragment key={idx}>{tok.value}</React.Fragment>;
        }
        return (
          <span
            key={idx}
            className={clsx('token', tok.type, styles[`tok_${tok.type}`])}
          >
            {tok.value}
          </span>
        );
      })}
    </code>
  );
}
