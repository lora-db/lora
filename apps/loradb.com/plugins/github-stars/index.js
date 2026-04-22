// Build-time star resolver. Runs once per build, fetches the current
// stargazer count from the GitHub REST API, and exposes the result via
// Docusaurus global data so the component can render it in the SSR HTML.
//
// Silent degradation: any error (network, rate limit, 404) leaves the
// value as null and the component falls back to its client-side resolver.
// If a GITHUB_TOKEN env var is present (CI builds), it is used to raise
// the rate-limit ceiling from 60/hr → 5000/hr.

const DEFAULT_REPO = 'lora-db/lora';
const FETCH_TIMEOUT_MS = 5000;

module.exports = function githubStarsPlugin(_context, options = {}) {
  const repo = options.repo || DEFAULT_REPO;

  return {
    name: 'github-stars',

    async loadContent() {
      const headers = { Accept: 'application/vnd.github+json', 'User-Agent': 'loradb-docs-build' };
      if (process.env.GITHUB_TOKEN) {
        headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
      }

      try {
        const res = await fetch(`https://api.github.com/repos/${repo}`, {
          headers,
          signal: AbortSignal.timeout(FETCH_TIMEOUT_MS),
        });
        if (!res.ok) {
          console.warn(`[github-stars] ${repo}: ${res.status} ${res.statusText}`);
          return { repo, stars: null, fetchedAt: null };
        }
        const data = await res.json();
        const stars = typeof data.stargazers_count === 'number' ? data.stargazers_count : null;
        if (stars != null) {
          console.log(`[github-stars] ${repo}: ${stars} stars`);
        }
        return { repo, stars, fetchedAt: Date.now() };
      } catch (err) {
        console.warn(`[github-stars] ${repo}: ${err.message}`);
        return { repo, stars: null, fetchedAt: null };
      }
    },

    async contentLoaded({ content, actions }) {
      actions.setGlobalData(content);
    },
  };
};
