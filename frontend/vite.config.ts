import { defineConfig } from "vite";
import { readFileSync } from "node:fs";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath, URL } from "node:url";
const developmentTarget =
  process.env.CUTTLEFISH_DEV_API ?? "http://127.0.0.1:64160";
const componentLicense = readFileSync(
  new URL("./THIRD_PARTY_NOTICES.md", import.meta.url),
  "utf8",
);
export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
  base: "./",
  define: {
    __APP_KIND__: JSON.stringify(mode === "platform" ? "platform" : "local"),
  },
  build: {
    outDir: mode === "platform" ? "../web" : "../web/local",
    emptyOutDir: false,
    cssCodeSplit: false,
    sourcemap: false,
    rolldownOptions: {
      output: {
        codeSplitting: false,
        banner: `/*!\n${componentLicense}\n*/`,
        entryFileNames: "app.js",
        assetFileNames: "app.[ext]",
      },
    },
  },
  server: {
    host: "127.0.0.1",
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": {
        target: developmentTarget,
        changeOrigin: true,
        configure(proxy) {
          proxy.on("proxyReq", (outgoing, incoming) => {
            // Only translate this loopback development UI's exact origin.
            if (incoming.headers.origin === "http://127.0.0.1:5173")
              outgoing.setHeader("Origin", new URL(developmentTarget).origin);
          });
        },
      },
    },
  },
}));
