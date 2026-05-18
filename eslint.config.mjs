// Root ESLint v9 flat config. Every workspace in the monorepo extends
// this — packages add their own override block only when they need it.
//
// Rough layering:
//   - global ignores      → never lint generated / vendored output
//   - js base             → eslint:recommended for all .js/.mjs/.cjs
//   - ts base             → typescript-eslint recommended for .ts/.tsx
//   - react overlay       → react + react-hooks rules for .tsx only
//   - storybook overlay   → relax a few rules inside *.stories.tsx
//   - test overlay        → vitest globals + permissive rules
//
// We intentionally do NOT enable type-checked rules at the root; they
// require a per-workspace tsconfig wiring that some packages (the
// Docusaurus app, the WASM glue) do not have today. Individual packages
// can opt in via their own config.

import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";
import reactPlugin from "eslint-plugin-react";
import reactHooks from "eslint-plugin-react-hooks";
import prettier from "eslint-config-prettier";

export default tseslint.config(
  {
    ignores: [
      "**/node_modules/**",
      "**/dist/**",
      "**/build/**",
      "**/storybook-static/**",
      "**/target/**",
      "**/wasm/**",
      "**/pkg-node/**",
      "**/pkg-bundler/**",
      "**/pkg-web/**",
      "**/.docusaurus/**",
      "**/.yarn/**",
      "**/tmp/**",
      "apps/loradb.com/static/**",
    ],
  },

  js.configs.recommended,

  ...tseslint.configs.recommended,

  {
    languageOptions: {
      ecmaVersion: 2023,
      sourceType: "module",
      globals: {
        ...globals.node,
        ...globals.browser,
      },
    },
    rules: {
      // Underscore prefix is the project-wide convention for
      // "intentionally unused" — covers callback signatures we keep
      // for documentation, generic params kept for symmetry, etc.
      "no-unused-vars": "off",
      "@typescript-eslint/no-unused-vars": [
        "error",
        {
          argsIgnorePattern: "^_",
          varsIgnorePattern: "^_",
          caughtErrorsIgnorePattern: "^_",
          destructuredArrayIgnorePattern: "^_",
          // Object-rest destructure (`const { a, ...rest } = x`) is the
          // canonical way to "strip these keys"; flagging the named
          // siblings as unused defeats the pattern.
          ignoreRestSiblings: true,
        },
      ],
    },
  },

  {
    files: ["**/*.{jsx,tsx}"],
    plugins: {
      react: reactPlugin,
      "react-hooks": reactHooks,
    },
    languageOptions: {
      parserOptions: {
        ecmaFeatures: { jsx: true },
      },
    },
    rules: {
      ...reactPlugin.configs.recommended.rules,
      ...reactHooks.configs.recommended.rules,
      "react/react-in-jsx-scope": "off",
      "react/prop-types": "off",
    },
    settings: {
      react: { version: "detect" },
    },
  },

  {
    files: ["**/*.stories.@(ts|tsx|js|jsx)"],
    rules: {
      "react-hooks/rules-of-hooks": "off",
      "@typescript-eslint/no-explicit-any": "off",
    },
  },

  {
    files: ["**/test/**", "**/tests/**", "**/*.test.@(ts|tsx|js|jsx)"],
    languageOptions: {
      globals: { ...globals.node, ...globals.browser },
    },
    rules: {
      "@typescript-eslint/no-explicit-any": "off",
    },
  },

  {
    files: ["**/*.cjs", "**/*.config.{js,cjs,mjs}", "**/scripts/**/*.{js,mjs,cjs}"],
    languageOptions: {
      sourceType: "commonjs",
      globals: { ...globals.node },
    },
    rules: {
      "@typescript-eslint/no-require-imports": "off",
    },
  },

  // Must come last: disables any ESLint rule that would conflict with
  // Prettier's formatting. Stylistic concerns belong to Prettier.
  prettier,
);
