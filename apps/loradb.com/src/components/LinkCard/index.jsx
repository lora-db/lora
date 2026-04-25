import React from 'react';
import clsx from 'clsx';
import Link from '@docusaurus/Link';

import styles from './styles.module.scss';

/**
 * Marketing routing primitive.
 *
 * A clickable card with an eyebrow tag, a title, a one-line
 * description, and a trailing arrow that animates on hover. Intended
 * for places where the page should send the reader somewhere
 * specific — intent routers, "where to next" grids, per-section
 * footers — instead of relying on inline prose links.
 *
 *   <LinkCard
 *     to="/docs/concepts/graph-model"
 *     eyebrow="Concept"
 *     title="The graph data model"
 *   >
 *     Labelled property graph, in process.
 *   </LinkCard>
 */
export default function LinkCard({
  to,
  href,
  eyebrow,
  title,
  children,
  className,
  variant = 'default',
}) {
  const target = to ?? href;
  return (
    <Link
      to={target}
      className={clsx(
        styles.linkCard,
        variant === 'compact' && styles.linkCardCompact,
        variant === 'accent' && styles.linkCardAccent,
        className,
      )}
    >
      {eyebrow ? <span className={styles.eyebrow}>{eyebrow}</span> : null}
      <span className={styles.title}>{title}</span>
      {children ? <span className={styles.body}>{children}</span> : null}
      <span className={styles.arrow} aria-hidden="true">
        <svg
          width="16"
          height="16"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <path d="M5 12h14M13 5l7 7-7 7" />
        </svg>
      </span>
    </Link>
  );
}
