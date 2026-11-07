import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { ErrorBoundary } from "./components/error-boundary";
import { createQueryClient } from "./lib/query";
import { LocalApp } from "./app/local-app";
import { PlatformApp } from "./app/platform-app";
import "./styles.css";
declare const __APP_KIND__: "local" | "platform";
const root = document.getElementById("root");
if (!root) throw new Error("Application root is missing");
createRoot(root).render(
  <StrictMode>
    <ErrorBoundary>
      <QueryClientProvider client={createQueryClient()}>
        {__APP_KIND__ === "platform" ? <PlatformApp /> : <LocalApp />}
      </QueryClientProvider>
    </ErrorBoundary>
  </StrictMode>,
);
