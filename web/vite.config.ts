import path from "node:path";
import { defineConfig } from "vite";

export default defineConfig({
  resolve: {
    alias: {
      "@adi/signaling-web-plugin/bus": path.resolve(
        __dirname,
        "../../signaling/web/src/bus/index.ts",
      ),
      "@adi/debug-screen-web-plugin/bus": path.resolve(
        __dirname,
        "../../debug-screen/web/src/bus/index.ts",
      ),
      "@adi/router-web-plugin/bus": path.resolve(
        __dirname,
        "../../router/web/src/bus/index.ts",
      ),
    },
  },
  build: {
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
