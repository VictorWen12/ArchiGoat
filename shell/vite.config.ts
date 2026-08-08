import { defineConfig } from "vite";

export default defineConfig({
  envPrefix: ["VITE_"],
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2022",
    emptyOutDir: true,
  },
});
