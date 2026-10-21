import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import boundaries from "eslint-plugin-boundaries";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "dist-demo", "storybook-static", "node_modules", "tests/e2e/.test-results", "tests/e2e/.html-report", "public/mockServiceWorker.js"] },
  {
    extends: [
      js.configs.recommended,
      ...tseslint.configs.recommendedTypeChecked,
    ],
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
      boundaries,
    },
    settings: {
      "import/resolver": {
        node: {
          extensions: [".js", ".jsx", ".ts", ".tsx"],
        },
      },
      "boundaries/elements": [
        { type: "app", pattern: "src/app/**" },
        { type: "feature", pattern: "src/features/*", capture: ["feature"] },
        { type: "shared", pattern: "src/shared/**" },
        // Top-level pages still hosting non-feature routes (NotFound, DetachedSession).
        { type: "page", pattern: "src/pages/**" },
      ],
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // React 19 + hooks plugin v7: compiler-oriented rules flag common sync-with-props
      // and shadcn patterns across the app; keep exhaustive-deps blocking CI.
      // A synchronous setState inside an effect renders the component twice per
      // run, so this is an error; a genuine external-system sync carries a
      // single-line disable with its reason instead.
      "react-hooks/set-state-in-effect": "error",
      "react-hooks/purity": "off",
      "react-hooks/exhaustive-deps": "error",
      "react-refresh/only-export-components": ["error", { allowConstantExport: true }],

      // Cheap to enforce: eslint autofixes these, or the fix is trivial.
      "@typescript-eslint/no-unused-vars": [
        "error",
        {
          argsIgnorePattern: "^_",
          varsIgnorePattern: "^_",
          caughtErrorsIgnorePattern: "^_",
          ignoreRestSiblings: true,
        },
      ],
      "@typescript-eslint/consistent-type-imports": [
        "error",
        { prefer: "type-imports", fixStyle: "separate-type-imports" },
      ],
      "@typescript-eslint/no-misused-promises": [
        "error",
        { checksVoidReturn: { attributes: false } },
      ],
      "@typescript-eslint/no-explicit-any": "error",

      // Typed rules: only-throw-error and unnecessary-type-assertion are errors.
      "@typescript-eslint/only-throw-error": "error",
      "@typescript-eslint/no-unnecessary-type-assertion": "error",

      // Cross-feature imports are forbidden so each feature stays self-contained;
      // code two features need belongs in src/shared/.
      "boundaries/no-unknown": "off",
      "boundaries/dependencies": [
        "error",
        {
          default: "allow",
          rules: [
            {
              from: { type: "feature" },
              disallow: { to: { type: "feature" } },
              message: "Cross-feature imports are forbidden; promote shared code to src/shared/.",
            },
            {
              from: { type: "shared" },
              disallow: { to: { type: ["feature", "app", "page"] } },
              message: "shared/* must not depend on features, app shell, or top-level pages.",
            },
          ],
        },
      ],

      "@typescript-eslint/no-floating-promises": "error",
      // apiClient is the typed openapi-fetch client (schema.gen.ts), so app code
      // has no `any` bleed to hide and the unsafe-* family is an error there;
      // test files opt out in the override below, because assertions routinely
      // poke at untyped API payloads.
      "@typescript-eslint/no-redundant-type-constituents": "error",
      "@typescript-eslint/no-unsafe-assignment": "error",
      "@typescript-eslint/no-unsafe-member-access": "error",
      "@typescript-eslint/no-unsafe-argument": "error",
      "@typescript-eslint/no-unsafe-call": "error",
      "@typescript-eslint/no-unsafe-return": "error",
      "@typescript-eslint/restrict-template-expressions": "error",
      "@typescript-eslint/no-base-to-string": "error",
      "@typescript-eslint/require-await": "error",
      // High-signal, 0 current violations in app code; cheap to keep green.
      "@typescript-eslint/no-unnecessary-boolean-literal-compare": "error",
      "@typescript-eslint/prefer-reduce-type-parameter": "error",
      // Union/enum switches must handle every case OR carry a default — catches
      // the "forgot a case AND no default → silent fall-through" bug class. A
      // deliberate `default` catch-all (common for filter-key/status maps here)
      // satisfies it, so well-formed switches aren't churned.
      "@typescript-eslint/switch-exhaustiveness-check": [
        "error",
        { considerDefaultExhaustiveForUnions: true },
      ],

      // Notifications flow through notify() so events also reach the in-app bell
      // feed. Direct sonner imports are banned outside the canonical wrapper
      // files (see override below for notifications.ts/sonner.tsx).
      "no-restricted-imports": [
        "error",
        {
          paths: [
            {
              name: "sonner",
              message:
                "Use notify() from shared/lib/notifications instead of importing sonner directly.",
            },
          ],
        },
      ],

      // Server-state reads must flow through canonical query helpers instead of
      // ad-hoc useEffect + fetch / *Api loops.
      "no-restricted-syntax": [
        "error",
        {
          selector:
            "CallExpression[callee.name='useEffect'] :function CallExpression[callee.name='fetch']",
          message:
            "useEffect+fetch is forbidden. Use useQuery(queries.*Options(...)) for server-state reads.",
        },
        {
          selector:
            "CallExpression[callee.name='useEffect'] :function CallExpression[callee.object.name=/.+Api$/]",
          message:
            "Calling `*Api.method()` from inside useEffect is forbidden. Reads belong in useQuery; mutations belong in useMutation or event handlers.",
        },
        // Native confirm()/alert() are unstyled, blocking browser dialogs; use
        // the AlertDialog primitive instead.
        {
          selector:
            "CallExpression[callee.name=/^(confirm|alert)$/]",
          message:
            "Use AlertDialog instead of native confirm()/alert().",
        },
        {
          selector:
            "CallExpression[callee.object.name='window'][callee.property.name=/^(confirm|alert)$/]",
          message:
            "Use AlertDialog instead of native confirm()/alert().",
        },
      ],
    },
  },
  {
    // The canonical notification wrapper and the Toaster setup are the two files
    // that legitimately import sonner — exempt only these from the sonner ban.
    files: [
      "src/shared/lib/notifications.ts",
      "src/shared/ui-primitives/sonner.tsx",
    ],
    rules: {
      "no-restricted-imports": "off",
    },
  },
  {
    files: ["src/**/*.test.{ts,tsx}", "src/**/*.spec.{ts,tsx}", "src/test/**/*.{ts,tsx}"],
    rules: {
      "@typescript-eslint/no-explicit-any": "off",
      "@typescript-eslint/no-floating-promises": "off",
      "@typescript-eslint/no-misused-promises": "off",
      "@typescript-eslint/no-unsafe-assignment": "off",
      "@typescript-eslint/no-unsafe-member-access": "off",
      "@typescript-eslint/no-unsafe-argument": "off",
      "@typescript-eslint/no-unsafe-call": "off",
      "@typescript-eslint/no-unsafe-return": "off",
    },
  },
  {
    // shadcn UI primitives are vendored from the shadcn registry. Each component
    // ships paired exports (e.g. Button + buttonVariants) by convention, and
    // splitting them defeats the registry round-trip.
    // Hooks-only files: every export is by definition a hook, not a component.
    // Context files (*Context.tsx): canonical React pattern is to colocate the
    // Context, its Provider, and the consumer hook in one module — splitting
    // them defeats the encapsulation guarantee that the Context is opaque.
    files: [
      "src/shared/ui-primitives/**/*.{ts,tsx}",
      "src/shared/hooks/**/*.{ts,tsx}",
      "src/**/*Context.tsx",
    ],
    rules: {
      "react-refresh/only-export-components": "off",
    },
  },
  {
    // Playwright specs are Node-side code (no browser globals, no React) and get
    // the same typed rule set as src, minus the unsafe-* family — the same
    // exemption src/**/*.test.ts carries, because assertions routinely poke at
    // untyped API payloads.
    files: ["tests/e2e/**/*.ts"],
    extends: [js.configs.recommended, ...tseslint.configs.recommendedTypeChecked],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.node,
      parserOptions: {
        project: ["./tsconfig.e2e.json"],
        tsconfigRootDir: import.meta.dirname,
      },
    },
    rules: {
      "@typescript-eslint/no-unused-vars": [
        "error",
        {
          argsIgnorePattern: "^_",
          varsIgnorePattern: "^_",
          caughtErrorsIgnorePattern: "^_",
          ignoreRestSiblings: true,
        },
      ],
      "@typescript-eslint/consistent-type-imports": [
        "error",
        { prefer: "type-imports", fixStyle: "separate-type-imports" },
      ],
      "@typescript-eslint/no-floating-promises": "error",
      "@typescript-eslint/only-throw-error": "error",
      "@typescript-eslint/no-explicit-any": "off",
      "@typescript-eslint/no-unsafe-assignment": "off",
      "@typescript-eslint/no-unsafe-member-access": "off",
      "@typescript-eslint/no-unsafe-argument": "off",
      "@typescript-eslint/no-unsafe-call": "off",
      "@typescript-eslint/no-unsafe-return": "off",
    },
  },
  {
    files: ["*.config.{ts,js,mjs}", "vite.config.ts", "vitest.config.ts", "playwright.config.ts", "scripts/**/*.mjs", "tailwind.config.ts", "postcss.config.js"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      globals: globals.node,
    },
  },
);
