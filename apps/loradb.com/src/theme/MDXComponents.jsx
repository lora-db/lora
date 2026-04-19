// Swizzle of the classic theme's MDX component registry.
//
// Anything exported here is available in every MDX (and `.md`) page
// without an explicit import. Keep this list short and well-scoped —
// MDX components leak into the global name space for prose writers.
//
// See https://docusaurus.io/docs/markdown-features/react#mdx-component-scope

import MDXComponents from '@theme-original/MDXComponents';
import CypherCode from '@site/src/components/CypherCode';

export default {
  ...MDXComponents,
  CypherCode,
};
