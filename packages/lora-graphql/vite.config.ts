import { defineConfig } from "vite";
import { resolve } from "node:path";
import dts from "vite-plugin-dts";

export default defineConfig({
  plugins: [
    dts({ entryRoot: "src", include: ["src/**/*"], rollupTypes: false }),
  ],
  build: {
    target: "es2022",
    sourcemap: true,
    lib: {
      entry: {
        index: resolve(__dirname, "src/index.ts"),
        cli: resolve(__dirname, "src/cli.ts"),
        testing: resolve(__dirname, "src/testing.ts"),
      },
      formats: ["es"],
      fileName: (_format, name) => `${name}.js`,
    },
    rollupOptions: {
      // graphql must stay a single shared instance with the host server.
      external: [/^graphql(\/.*)?$/, /^node:/, /^@loradb\/lora-node$/],
      output: {
        banner: (chunk) => (chunk.name === "cli" ? "#!/usr/bin/env node" : ""),
      },
    },
  },
});
