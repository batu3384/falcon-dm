import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// ponytail: Vitest config. jsdom environment so React Testing Library can render
// components; the same Vite plugin resolves TS/JSX. Coverage target is the
// critical path (api, store, schema, format utils).
export default defineConfig({
  plugins: [react()],
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
    coverage: {
      provider: "v8",
      reporter: ["text", "html"],
      include: ["src/api/**", "src/store/**", "src/lib/**", "src/components/ErrorBoundary.tsx"],
    },
  },
});
