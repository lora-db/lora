module.exports = {
  extends: [
    "next/core-web-vitals",
    "next/typescript",
    // Runs prettier as an ESLint rule (auto-fixable) and disables any
    // core rule that would conflict with prettier's formatting. Listed
    // last so it wins. `next lint --fix` and IDE fix-on-save will apply
    // prettier formatting alongside other ESLint fixes.
    "plugin:prettier/recommended",
  ],
  rules: {
    // Mirror the root config: underscore prefix is the project-wide
    // convention for "intentionally unused".
    "@typescript-eslint/no-unused-vars": [
      "error",
      {
        argsIgnorePattern: "^_",
        varsIgnorePattern: "^_",
        caughtErrorsIgnorePattern: "^_",
        destructuredArrayIgnorePattern: "^_",
        ignoreRestSiblings: true,
      },
    ],
  },
};
