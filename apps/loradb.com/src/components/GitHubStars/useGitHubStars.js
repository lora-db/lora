import { useEffect, useState } from 'react';
import { usePluginData } from '@docusaurus/useGlobalData';

// Resolver layering:
//   1. Build-time (plugins/github-stars → globalData) — SSR-visible, no flash
//   2. localStorage cache (1h TTL) — fast hydration across navigations
//   3. Background fetch to api.github.com — refreshes stale values
//
// Each layer is optional. If every layer fails (offline first visit, API
// rate limited, build couldn't reach GitHub), the consumer gets `null`
// and is expected to render gracefully without a count.

const CACHE_KEY = 'loradb:github-stars';
const TTL_MS = 60 * 60 * 1000;

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

async function fetchStars(repo, signal) {
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

export function formatCount(n) {
  if (n == null) return null;
  if (n < 1000) return String(n);
  if (n < 10_000) return (n / 1000).toFixed(1).replace(/\.0$/, '') + 'k';
  if (n < 1_000_000) return Math.round(n / 1000) + 'k';
  return (n / 1_000_000).toFixed(1).replace(/\.0$/, '') + 'M';
}

export default function useGitHubStars(repo) {
  const buildData = usePluginData('github-stars');
  const buildStars =
    buildData && buildData.repo === repo && typeof buildData.stars === 'number'
      ? buildData.stars
      : null;

  const [stars, setStars] = useState(buildStars);

  useEffect(() => {
    const cached = readCache();
    // Stargazing is monotonic in practice — prefer the higher of build-time
    // and cached so we never regress the displayed count across navigations.
    if (cached && cached.repo === repo) {
      if (buildStars == null || cached.stars > buildStars) {
        setStars(cached.stars);
      }
    }

    const controller = new AbortController();
    const needsFetch = !cached || cached.stale || cached.repo !== repo;
    if (needsFetch) {
      fetchStars(repo, controller.signal)
        .then(setStars)
        .catch(() => {
          /* keep whatever we already have — build-time, cache, or nothing */
        });
    }
    return () => controller.abort();
  }, [repo, buildStars]);

  return stars;
}
