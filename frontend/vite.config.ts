import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { resolve } from "node:path";

export default defineConfig({
  plugins: [svelte(), tailwindcss()],
  build: {
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
        setup: resolve(__dirname, "setup.html"),
      },
    },
  },
  server: {
    proxy: {
      "/ws": {
        target: "http://localhost:9800",
        ws: true,
      },
      "/api": {
        target: "http://localhost:9800",
      },
      // "/proxy" removed (security review 2026-08-02, SR-01): the reverse
      // proxy is behind the server's `webviews` feature and off by default.
    },
  },
});
