// Swizzle of the classic theme's MDX component registry.
//
// Anything exported here is available in every MDX (and `.md`) page
// without an explicit import. Keep this list short and well-scoped —
// MDX components leak into the global name space for prose writers.
//
// See https://docusaurus.io/docs/markdown-features/react#mdx-component-scope

import MDXComponents from "@theme-original/MDXComponents";
import BenchmarkSummary from "@site/src/components/BenchmarkSummary";
import CypherCode from "@site/src/components/CypherCode";
import CypherSnippet from "@site/src/components/CypherSnippet";
import Definition from "@site/src/components/Definition";
import FAQ from "@site/src/components/FAQ";
import HowTo from "@site/src/components/HowTo";
import QueryCodeBlock from "@site/src/components/LoraQueryCodeBlock";

export default {
  ...MDXComponents,
  BenchmarkSummary,
  CypherCode,
  CypherSnippet,
  Definition,
  FAQ,
  HowTo,
  QueryCodeBlock,
};
