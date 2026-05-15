import type { StorybookConfig } from "@storybook/react-vite";
import { mergeConfig } from "vite";
import wasm from "vite-plugin-wasm";
import topLevelAwait from "vite-plugin-top-level-await";

const config: StorybookConfig = {
  stories: ["../src/**/*.mdx", "../src/**/*.stories.@(ts|tsx)"],
  addons: [
    "@storybook/addon-links",
    "@storybook/addon-essentials",
    "@storybook/addon-interactions",
  ],
  framework: {
    name: "@storybook/react-vite",
    options: {},
  },
  docs: { autodocs: "tag" },
  // The WASM parser depends on top-level await + wasm-bindgen output, so
  // Storybook's Vite needs the matching plugins. Reuse the same pair the
  // library build uses.
  async viteFinal(viteConfig) {
    return mergeConfig(viteConfig, {
      plugins: [wasm(), topLevelAwait()],
    });
  },
};

export default config;
