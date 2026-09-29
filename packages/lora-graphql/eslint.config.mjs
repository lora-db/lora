// Per-package ESLint config that pulls in the workspace root config.
// Kept as a thin re-export so `yarn workspace … lint` and editor LSPs
// resolve a config file inside this package, while the rules themselves
// live in the monorepo root.

import root from "../../eslint.config.mjs";

export default root;
