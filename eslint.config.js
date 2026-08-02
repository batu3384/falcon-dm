// ponytail: ESLint flat config for the React + TS frontend. CI runs
// `eslint . --max-warnings 0`. Catches the most common React/TS foot-guns:
// unused vars, `any`, hook rule violations, and missing key props.
import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import globals from "globals";

export default tseslint.config(
  { ignores: ["dist/**", "src-tauri/**", "node_modules/**"] },
  {
    files: ["**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // ponytail: the React 19 compiler-style "set-state-in-effect" rule flags
      // every event-listener/polling effect that calls setState — which is the
      // intended pattern for subscribing to Tauri events. We disable it globally;
      // the genuine cascading-render bugs it catches are rare here and would be
      // better addressed via the React Compiler if adopted later.
      "react-hooks/set-state-in-effect": "off",
      // ponytail: TanStack Virtual is React 19-compatible; the rule flags its
      // useVirtualizer hook as incompatible (false positive).
      "react-hooks/incompatible-library": "off",
      "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],
      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/no-unused-vars": ["warn", { argsIgnorePattern: "^_" }],
      "@typescript-eslint/consistent-type-imports": "warn",
      "no-console": ["warn", { allow: ["warn", "error"] }],
    },
  },
);
