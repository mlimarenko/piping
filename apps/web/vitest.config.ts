import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "path";

export default defineConfig({
  plugins: [react()],
  server: {
    fs: {
      // `src/demo/handlers.ts` imports the generated OpenAPI document with
      // `?raw` from outside this package. A production build resolves that
      // through rollup and never consults this list, so only the transform
      // pipeline needs telling - which is why a test importing the demo
      // handlers failed with "Denied ID" while the demo itself built fine.
      // This file does not inherit vite.config.ts, so the allowance belongs here.
      allow: [path.resolve(__dirname, "../..")],
    },
  },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
    coverage: {
      provider: "v8",
      reporter: ["lcov", "text-summary"],
      reportsDirectory: "coverage",
      include: ["src/**/*.{ts,tsx}"],
      exclude: ["src/**/*.{test,spec}.{ts,tsx}", "src/test/**", "src/**/*.gen.ts"],
      // A threshold makes coverage a gate rather than a number forwarded to
      // Sonar. The floors sit just below the measured develop baseline
      // (quality:frontend:coverage job 92944, pipeline 11143: statements 46.98,
      // branches 43.81, functions 38.17, lines 48.62), so an ordinary change
      // passes while a real drop fails the job. Raise them with the baseline;
      // never lower a floor to make a red job green.
      thresholds: {
        statements: 46,
        branches: 43,
        functions: 37,
        lines: 48,
      },
    },
  },
  resolve: {
    alias: { "@": path.resolve(__dirname, "./src") },
  },
});
