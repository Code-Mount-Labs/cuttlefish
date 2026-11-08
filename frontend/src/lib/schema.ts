import { z } from "zod";
// Production CSP forbids runtime code generation, including validation probes.
z.config({ jitless: true });
export { z };
