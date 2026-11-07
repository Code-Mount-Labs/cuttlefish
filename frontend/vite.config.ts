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
export default defineConfig(({ mode }) => {
  const platform = mode.endsWith("platform");
  const embedded = mode.startsWith("embedded-");
  return {
    plugins: [react(), tailwindcss()],
    resolve: {
      alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
    },
    base: embedded ? "./" : "/",
    define: {
      __APP_KIND__: JSON.stringify(platform ? "platform" : "local"),
    },
    build: {
      outDir: embedded
        ? platform
          ? "../web"
          : "../web/local"
        : platform
          ? "dist/platform"
          : "dist",
      emptyOutDir: !embedded,
      // Embedded binaries have no asset directory; bundle their local fonts.
      assetsInlineLimit: embedded ? 200_000 : 4096,
      cssCodeSplit: !embedded,
      sourcemap: false,
      rolldownOptions: {
        output: {
          codeSplitting: !embedded,
          banner: `/*!\n${componentLicense}\n*/`,
          entryFileNames: embedded ? "app.js" : "assets/[name]-[hash].js",
          assetFileNames: embedded ? "app.[ext]" : "assets/[name]-[hash].[ext]",
        },
      },
    },
    preview: {
      host: "127.0.0.1",
      port: 4173,
      strictPort: true,
      proxy: {
        "/api": {
          target: developmentTarget,
          changeOrigin: true,
          configure(proxy) {
            proxy.on("proxyReq", (outgoing, incoming) => {
              if (incoming.headers.origin === "http://127.0.0.1:4173")
                outgoing.setHeader("Origin", new URL(developmentTarget).origin);
            });
          },
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
  };
});
