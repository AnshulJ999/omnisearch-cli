import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

// `npm run dev` serves the page with live reload and sends /api to a running
// `osearch ui` (port 7820 unless OSEARCH_UI_PORT says otherwise).
const api = `http://127.0.0.1:${process.env.OSEARCH_UI_PORT ?? "7820"}`;

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.resolve(__dirname, "src") } },
  server: {
    port: 5173,
    proxy: {
      "/api": {
        target: api,
        changeOrigin: true,
        // osearch only answers its own host and origin.
        headers: { host: api.replace("http://", ""), origin: api },
      },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    chunkSizeWarningLimit: 1500,
    rollupOptions: {
      output: {
        manualChunks: {
          charts: ["echarts", "echarts-for-react"],
          flow: ["@xyflow/react", "elkjs/lib/elk.bundled.js"],
          markdown: ["react-markdown", "remark-gfm"],
        },
      },
    },
  },
});
