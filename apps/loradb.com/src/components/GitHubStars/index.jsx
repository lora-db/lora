import React, { useEffect, useState } from 'react';
import clsx from 'clsx';
import Link from '@docusaurus/Link';
import { usePluginData } from '@docusaurus/useGlobalData';

import styles from './styles.module.scss';

// Stars are resolved client-side against the unauthenticated GitHub API
// (60 req / hour / IP). A localStorage cache with a 1-hour TTL keeps us
// well under that limit even for returning visitors.
const CACHE_KEY = 'loradb:github-stars';
const TTL_MS = 60 * 60 * 1000;

function formatCount(n) {
  if (n == null) return null;
  if (n < 1000) return String(n);
  if (n < 10_000) return (n / 1000).toFixed(1).replace(/\.0$/, '') + 'k';
  if (n < 1_000_000) return Math.round(n / 1000) + 'k';
  return (n / 1_000_000).toFixed(1).replace(/\.0$/, '') + 'M';
}

function readCache() {
  if (typeof window === 'undefined') return null;
  try {
    const raw = window.localStorage.getItem(CACHE_KEY);
    if (!raw) return null;
    const { stars, ts, repo } = JSON.parse(raw);
    if (typeof stars !== 'number') return null;
    return { stars, repo, stale: Date.now() - ts > TTL_MS };
  } catch {
    return null;
  }
}

function writeCache(repo, stars) {
  if (typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(
      CACHE_KEY,
      JSON.stringify({ repo, stars, ts: Date.now() }),
    );
  } catch {
    /* storage disabled — ignore */
  }
}

// Resolver: returns the cached value if fresh, otherwise fetches and
// writes back. Silently no-ops on network or parse errors so a rate
// limit or an offline user never breaks the navbar.
async function resolveStars(repo, signal) {
  const res = await fetch(`https://api.github.com/repos/${repo}`, {
    headers: { Accept: 'application/vnd.github+json' },
    signal,
  });
  if (!res.ok) throw new Error(`github api ${res.status}`);
  const data = await res.json();
  if (typeof data.stargazers_count !== 'number') {
    throw new Error('unexpected github api payload');
  }
  writeCache(repo, data.stargazers_count);
  return data.stargazers_count;
}

export default function GitHubStars({
  repo = 'lora-db/lora',
  href = 'https://github.com/lora-db/lora',
  label = 'GitHub repository',
  // Docusaurus passes the whole navbar item through — absorb the extras
  // that would otherwise leak to the DOM and trip React warnings.
  position, // eslint-disable-line no-unused-vars
  ...rest
}) {
  // Build-time resolver (plugins/github-stars) runs once per build and
  // exposes the star count through global data. Using it as the initial
  // state means SSR already ships the real count → no flash, and the
  // page still works for visitors the GitHub API is rate-limiting.
  const buildData = usePluginData('github-stars');
  const buildStars =
    buildData && buildData.repo === repo && typeof buildData.stars === 'number'
      ? buildData.stars
      : null;

  const [stars, setStars] = useState(buildStars);

  useEffect(() => {
    const cached = readCache();
    // Prefer the freshest known value: cache beats build-time only if the
    // cached count is higher (stargazing is monotonic in practice) or if
    // we didn't get a build-time value.
    if (cached && cached.repo === repo) {
      if (buildStars == null || cached.stars > buildStars) {
        setStars(cached.stars);
      }
    }

    const controller = new AbortController();
    const needsFetch = !cached || cached.stale || cached.repo !== repo;
    if (needsFetch) {
      resolveStars(repo, controller.signal)
        .then(setStars)
        .catch(() => {
          /* keep whatever we already have — build-time, cache, or nothing */
        });
    }
    return () => controller.abort();
  }, [repo, buildStars]);

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
