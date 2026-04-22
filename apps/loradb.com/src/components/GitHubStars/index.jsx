import React from 'react';
import clsx from 'clsx';
import Link from '@docusaurus/Link';

import useGitHubStars, { formatCount } from './useGitHubStars';
import styles from './styles.module.scss';

// Compact navbar pill. The expanded-with-count state is desktop-only
// (see styles.module.scss); on tablet/mobile it collapses back to a
// 34x34 icon chiclet that matches the neighbouring Discord / X icons.
export default function GitHubStars({
  repo = 'lora-db/lora',
  href = 'https://github.com/lora-db/lora',
  label = 'GitHub repository',
  position, // eslint-disable-line no-unused-vars — swallow Docusaurus navbar item prop
  ...rest
}) {
  const stars = useGitHubStars(repo);
  const showCount = stars != null;
  const formatted = formatCount(stars);

  return (
    <Link
      to={href}
      className={clsx(styles.root, showCount && styles.withCount)}
      aria-label={showCount ? `${label} — ${stars} stars` : label}
      target="_blank"
      rel="noopener noreferrer"
      {...rest}
    >
      <span className={styles.icon} aria-hidden="true" />
      {showCount && (
        <>
          <span className={styles.divider} aria-hidden="true" />
          <span className={styles.count}>
            <svg
              className={styles.star}
              viewBox="0 0 16 16"
              aria-hidden="true"
              focusable="false"
            >
              <path d="M8 .25a.75.75 0 0 1 .673.418l1.882 3.815 4.21.612a.75.75 0 0 1 .416 1.279l-3.046 2.97.72 4.192a.75.75 0 0 1-1.088.791L8 12.347l-3.767 1.98a.75.75 0 0 1-1.088-.79l.72-4.194L.818 6.374a.75.75 0 0 1 .416-1.28l4.21-.61L7.327.668A.75.75 0 0 1 8 .25z" />
            </svg>
            <span className={styles.countValue}>{formatted}</span>
          </span>
        </>
      )}
    </Link>
  );
}
