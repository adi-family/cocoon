import path from "node:path";
import { defineConfig } from "vite";

export default defineConfig({
  resolve: {
    alias: {
      "@adi-family/plugin-signaling/bus": path.resolve(
        __dirname,
        "../../signaling/web/src/bus/index.ts",
      ),
      "@adi-family/plugin-debug-screen/bus": path.resolve(
        __dirname,
        "../../debug-screen/web/src/bus/index.ts",
      ),
      "@adi-family/plugin-router/bus": path.resolve(
        __dirname,
        "../../router/web/src/bus/index.ts",
      ),
    },
  },
  build: {
    outDir: "../../../dist/cocoon",
    lib: {
      entry: "src/index.ts",
      formats: ["es"],
      fileName: () => "web.js",
    },
    rollupOptions: {
      external: ["@adi-family/sdk-plugin"],
      output: {
        inlineDynamicImports: true,
        assetFileNames: "style[extname]",
      },
    },
    target: "es2022",
    minify: true,
  },
});
