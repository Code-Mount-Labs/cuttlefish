import "@testing-library/jest-dom/vitest";
import { afterEach, vi } from "vitest";
import { cleanup } from "@testing-library/react";
import { webcrypto } from "node:crypto";
import { Blob, File } from "node:buffer";
Object.defineProperty(globalThis, "crypto", {
  value: webcrypto,
  configurable: true,
});
Object.defineProperty(globalThis, "Blob", { value: Blob, configurable: true });
Object.defineProperty(globalThis, "File", { value: File, configurable: true });
Object.defineProperty(window, "matchMedia", {
  value: vi.fn(() => ({
    matches: false,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  })),
  writable: true,
});
URL.createObjectURL = vi.fn(() => "blob:preview");
URL.revokeObjectURL = vi.fn();
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  sessionStorage.clear();
  history.replaceState(null, "", "/");
});
