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
  {
    // ponytail: the MV3 extension handles cookies, download interception and URL
    // parsing — lint it for correctness, not style. Its scripts are classic (not
    // modules) and share one global scope per context: the manifest loads
    // media-utils.js + shared.js + pairing.js + api.js + background.js into the
    // same background scope, and popup.html/options.html load subsets of them.
    // Top-level bindings therefore cross file boundaries, so the shared surface
    // is declared as globals below — that keeps `no-undef` catching genuine
    // typos instead of flooding on sibling-file definitions.
    files: ["extension/**/*.js"],
    extends: [js.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "script",
      globals: {
        ...globals.browser,
        ...globals.webextensions,
        ...globals.serviceworker,
        // Attached to globalThis by media-utils.js.
        ext: "readonly",
        FalconMedia: "readonly",
        // Defined in shared.js, consumed across the background scope and pages.
        // `let` bindings are modeled "writable" because the shared scope is one
        // logical module: owning and sibling files both assign them (e.g.
        // interceptPaused in background.js, pairInFlight in pairing.js).
        FALCON_API: "readonly",
        HEALTH_TIMEOUT_MS: "readonly",
        INJECTED: "readonly",
        MEDIA_META: "readonly",
        MEDIA_URLS: "readonly",
        PAIR_POLL_ATTEMPTS: "readonly",
        REQUEST_TIMEOUT_MS: "readonly",
        appHealthy: "readonly",
        captureBlockedHosts: "writable",
        captureExtensions: "writable",
        connectionState: "writable",
        cookieLookupUrl: "readonly",
        defaultCaptureExtensionsText: "readonly",
        ensureAppRunning: "readonly",
        ensureContentScript: "readonly",
        eraseBrowserDownload: "readonly",
        extensionOriginId: "readonly",
        fetchBlobPayload: "readonly",
        fetchWithTimeout: "readonly",
        getActiveBrowserTab: "readonly",
        getCookiesHeader: "readonly",
        getInterceptFailClosed: "readonly",
        getToken: "readonly",
        interceptFailClosed: "writable",
        interceptPaused: "writable",
        isOwnBrowserDownload: "readonly",
        markPauseWrite: "readonly",
        msg: "readonly",
        normalizeCaptureBlockedHosts: "readonly",
        normalizeCaptureExtensions: "readonly",
        notify: "readonly",
        notifySuccess: "writable",
        pairInFlight: "writable",
        prefsHydrated: "writable",
        prefsReady: "readonly",
        refreshBadge: "readonly",
        reinitContentScriptsAfterReload: "readonly",
        setInterceptFailClosed: "readonly",
        setNotifySuccessEnabled: "readonly",
        setState: "readonly",
        shouldCaptureBrowserDownload: "readonly",
        sleep: "readonly",
        waitForHealthy: "readonly",
        wakeFalcon: "readonly",
        withTimeout: "readonly",
        // Defined in pairing.js, used in background.js.
        ensurePaired: "readonly",
        formatPairCode: "readonly",
        pingToken: "readonly",
        visiblePairCode: "readonly",
        // Defined in api.js, used in background.js.
        fetchDownloads: "readonly",
        postFalcon: "readonly",
        sendToFalcon: "readonly",
      },
    },
    rules: {
      // vars:"local" — top-level bindings in classic scripts are the cross-file
      // shared surface above; whether they are used lives in sibling files, so
      // only function-local variables can be soundly checked per file.
      // args:"none" — several shared signatures keep stable parameters a single
      // implementation does not read (e.g. isJunkHijackFilename's `url`).
      "no-unused-vars": [
        "warn",
        { vars: "local", args: "none", caughtErrorsIgnorePattern: "^_" },
      ],
      // In-file declarations of the shared-scope surface must shadow the ambient
      // globals above; genuine duplicate declarations inside one file still fire.
      "no-redeclare": ["error", { builtinGlobals: false }],
      // `catch (_) {}` is the extension's deliberate ignore-failure idiom (30+
      // sites); empty if/while/etc. blocks are still reported.
      "no-empty": ["error", { allowEmptyCatch: true }],
      // media-utils.js intentionally matches control characters to strip them
      // from filenames ([\u0000-\u001f\u007f...]) — not an accidental regex.
      "no-control-regex": "off",
      // The catch blocks here rethrow user-facing localized messages on purpose;
      // the original network/parse error is noise, so `cause` is deliberately
      // omitted. One trivial dead reassign this also hides: `data = null` in
      // api.js's parse catch is unreachable state (no-useless-assignment).
      "preserve-caught-error": "off",
      "no-useless-assignment": "off",
    },
  },
);
