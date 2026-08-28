import tailwind from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";
import { defineConfig } from "vite";

export default defineConfig({
  root: __dirname,
  base: "./",
  plugins: [react(), tailwind()],
  resolve: {
    alias: {
      // Protocol types are generated from the Rust source by `cargo test`
      // (ts-rs) and synced in by `protocol:sync`. The Client never
      // hand-writes wire types.
      "@protocol": resolve(__dirname, "src/protocol"),
    },
  },
  build: {
    outDir: "dist/renderer",
    emptyOutDir: true,
  },
});
