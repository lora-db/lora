import React from 'react';
import clsx from 'clsx';
import Link from '@docusaurus/Link';

import useGitHubStars, { formatCount } from '@site/src/components/GitHubStars/useGitHubStars';
import styles from './styles.module.scss';

// Body-copy CTA. Two zones:
//   • Left  — GitHub mark + "Star on GitHub" label (the ask)
//   • Right — ★ + live star count (social proof)
//
// The star icon nudges on hover so the affordance reads as "click the
// star" rather than "click the button". When the count isn't resolved
// (offline + no build-time value), the right zone hides cleanly and
// the left zone stays as a standalone CTA.
export default function StarOnGitHub({
  repo = 'lora-db/lora',
  href = 'https://github.com/lora-db/lora',
  label = 'Github',
  size = 'md',
  className,
}) {
  const stars = useGitHubStars(repo);
  const showCount = stars != null;
  const formatted = formatCount(stars);

  return (
    <Link
      to={href}
      target="_blank"
      rel="noopener noreferrer"
      aria-label={showCount ? `${label} — ${stars} stars` : label}
      className={clsx(styles.root, styles[`size-${size}`], className)}
    >
      <span className={styles.cta}>
        <svg
          className={styles.gh}
          viewBox="0 0 24 24"
          aria-hidden="true"
          focusable="false"
        >
          <path
            fill="currentColor"
            d="M12 .5A11.5 11.5 0 0 0 .5 12c0 5.08 3.29 9.39 7.86 10.91.58.11.79-.25.79-.56v-2c-3.2.7-3.87-1.37-3.87-1.37-.52-1.33-1.28-1.69-1.28-1.69-1.05-.72.08-.7.08-.7 1.16.08 1.77 1.2 1.77 1.2 1.03 1.77 2.71 1.26 3.37.96.1-.75.4-1.26.73-1.55-2.55-.29-5.23-1.28-5.23-5.7 0-1.26.45-2.28 1.2-3.09-.12-.29-.52-1.47.11-3.06 0 0 .97-.31 3.18 1.18a11 11 0 0 1 5.79 0c2.21-1.49 3.18-1.18 3.18-1.18.63 1.59.23 2.77.11 3.06.75.81 1.2 1.83 1.2 3.09 0 4.43-2.69 5.41-5.25 5.69.41.35.78 1.04.78 2.1v3.11c0 .31.21.67.8.56A11.5 11.5 0 0 0 23.5 12 11.5 11.5 0 0 0 12 .5z"
          />
        </svg>
        <span className={styles.ctaLabel}>{label}</span>
      </span>

      {showCount && (
        <>
          <span className={styles.divider} aria-hidden="true" />
          <span className={styles.countZone}>
            <svg
              className={styles.star}
              viewBox="0 0 24 24"
              aria-hidden="true"
              focusable="false"
            >
              <path
                fill="currentColor"
                d="M12 2.25a.9.9 0 0 1 .807.5l2.604 5.276 5.823.846a.9.9 0 0 1 .5 1.536l-4.214 4.108 1 5.8a.9.9 0 0 1-1.306.949L12 18.497l-5.214 2.74a.9.9 0 0 1-1.306-.948l.995-5.8-4.214-4.108a.9.9 0 0 1 .499-1.537l5.823-.846L11.193 2.75a.9.9 0 0 1 .807-.5z"
              />
            </svg>
            <span className={styles.countValue}>{formatted}</span>
          </span>
        </>
      )}
    </Link>
  );
}
