import { fileURLToPath, URL } from 'node:url';
import tailwindcss from '@tailwindcss/vite';
import vue from '@vitejs/plugin-vue';
import { defineConfig } from 'vite-plus';
import VueRouter from 'vue-router/vite';

/**
 * Paths nothing should lint or format: build output, Rust, and generated files whose generator's
 * formatting is the contract.
 */
const IGNORE_PATTERNS = [
  '**/dist/**',
  '**/node_modules/**',
  '**/target/**',
  '**/coverage/**',
  'src-tauri/gen/**',
  // ts-rs writes this; a formatter rewriting it makes every regeneration produce a diff that
  // reflects nothing, and CI's "regenerate and fail on a diff" check stops meaning anything.
  'src/scripts/generated/**',
  // Emitted by vue-router/vite. Committed so the build does not depend on plugin ordering.
  'src/types/route-map.d.ts',
  'pnpm-lock.yaml',
];

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [VueRouter({ dts: 'src/types/route-map.d.ts' }), vue(), tailwindcss()],

  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },

  // Do not obscure Rust compiler errors during `tauri dev`.
  clearScreen: false,

  server: {
    // Tauri expects a fixed port and fails if it is taken, rather than silently moving.
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: {
      ignored: ['**/src-tauri/**', '**/crates/**', '**/target/**'],
    },
  },

  build: {
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    // NOT 'esbuild'. That value is deprecated in Vite 8 and slated for removal; Oxc is the
    // minifier. Tauri's own Vite guide still documents the old value.
    minify: 'oxc',
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },

  // `envPrefix` is a literal startsWith test, NOT a glob. Tauri's Vite guide documents
  // `'TAURI_ENV_*'`, whose trailing `*` matches no variable at all, so
  // `import.meta.env.TAURI_ENV_PLATFORM` comes out `undefined`. Config-side `process.env` reads
  // work either way, which is exactly why the mistake goes unnoticed.
  envPrefix: ['VITE_', 'TAURI_ENV_'],

  test: {
    environment: 'jsdom',
    // Threads, not the default forks: measured at half the wall clock on this suite (10s against
    // 20s), and nothing here needs process isolation.
    pool: 'threads',
    include: ['src/**/*.{test,spec}.{ts,vue}'],
    setupFiles: ['src/tests/setup.ts'],
    // Reset mock state between tests here rather than in each file. `ipc.ts` is stateless by
    // design so that `clearMocks` alone is enough to isolate a test — this is the config half of
    // that bargain, and without it isolation depends on every file remembering to do it.
    clearMocks: true,
    restoreMocks: true,
  },

  lint: {
    plugins: [
      'eslint',
      'typescript',
      'oxc',
      'unicorn',
      'import',
      'vue',
      'promise',
      'jsx-a11y',
      'jsdoc',
    ],
    // Vite+ ships its own oxlint JS plugin enforcing "import configs from `vite-plus`, tests from
    // `vite-plus/test` — never `vite` / `vitest` direct". It is a JS custom plugin, so it loads
    // via jsPlugins rather than the native `plugins` list above.
    jsPlugins: [{ name: 'vite-plus', specifier: 'vite-plus/oxlint-plugin' }],
    // Type-aware linting and TypeScript diagnostics, both via oxlint-tsgolint. This is what makes
    // `vp check` a type check for plain `.ts`; `.vue` goes through vue-tsc in the typecheck task.
    options: { typeAware: true, typeCheck: true },
    categories: {
      correctness: 'error',
      suspicious: 'error',
      perf: 'error',
      // Pedantic produces style suggestions we do not act on.
      pedantic: 'off',
    },
    env: { browser: true, node: true, vue: true, vitest: true },
    ignorePatterns: IGNORE_PATTERNS,
    rules: {
      'eslint/no-unused-vars': [
        'error',
        {
          argsIgnorePattern: '^_',
          destructuredArrayIgnorePattern: '^_',
          varsIgnorePattern: '^_',
        },
      ],
      'no-console': ['error', { allow: ['warn', 'error', 'info'] }],
      // `__TAURI_INTERNALS__` is Tauri's own global. The test harness in `src/tests/channel.ts`
      // drives a mocked `Channel` through it, and the name is not ours to choose. Named explicitly
      // rather than disabled inline, so the exemption is one identifier and not one file.
      'no-underscore-dangle': ['error', { allow: ['__TAURI_INTERNALS__'] }],
      eqeqeq: ['error', 'smart'],
      // Vue SFCs and vite config files export defaults.
      'import/no-default-export': 'off',
      // Vue SFCs are PascalCase; kebab breaks discovery.
      'unicorn/filename-case': 'off',
      // TypeScript owns types; JSDoc type tags are noise in .ts.
      'jsdoc/require-param-type': 'off',
      'jsdoc/require-property-type': 'off',
      'jsdoc/require-returns-type': 'off',
      // Trailing comments in this codebase are WHY-notes, not WHAT-noise.
      'no-inline-comments': 'off',
      // Side-effect imports that are meant to be side effects; CSS is required by the bundler.
      'import/no-unassigned-import': ['error', { allow: ['**/*.css'] }],
      // TODO/XXX markers are tracked deliberately.
      'no-warning-comments': 'off',
      'vite-plus/prefer-vite-plus-imports': 'error',
    },
    overrides: [
      {
        files: ['**/*.test.ts', '**/*.spec.ts', '**/tests/**/*.ts'],
        rules: {
          // Mocks and fixtures commonly use `any` for brevity.
          'typescript/no-explicit-any': 'off',
          // Test files aggregate many assertions — size caps do not apply.
          'eslint/max-lines': 'off',
          'eslint/max-lines-per-function': 'off',
          // Test helpers' signatures are self-evident.
          'jsdoc/require-param': 'off',
          'jsdoc/require-returns': 'off',
          // `fn(undefined)` and `toBe(undefined)` are vitest-native.
          'unicorn/no-useless-undefined': 'off',
          // `it('…', async () => {})` signature uniformity.
          'eslint/require-await': 'off',
        },
      },
      {
        // Node tooling scripts — not app code.
        files: ['tools/scripts/**'],
        env: { node: true, browser: false },
        rules: {
          'no-console': 'off',
          'unicorn/no-process-exit': 'off',
        },
      },
    ],
  },

  fmt: {
    singleQuote: true,
    sortImports: {
      internalPattern: ['@/'],
      newlinesBetween: false,
    },
    sortTailwindcss: {
      functions: ['clsx', 'cn', 'cva', 'tw'],
    },
    jsdoc: {
      commentLineStrategy: 'multiline',
      descriptionWithDot: true,
    },
    ignorePatterns: IGNORE_PATTERNS,
  },

  // Formats and lints whatever is being committed. The pre-push hook in .vite-hooks/ is the
  // cold gate on top of this; both are installed by `vp config`, run from the `prepare` script.
  staged: { '*': 'vp check --fix' },

  run: {
    tasks: {
      // Long-running. cache:false, or the runner replays captured stdout and never starts a window.
      dev: { command: 'tauri dev', cache: false },
      // `vp check` does not route Vue SFCs through vue-tsc, so SFC type-checking stays its own
      // task and `build` depends on it. Plain `.ts` — this file included — is already type-checked
      // by `vp check` through tsgolint's `typeCheck`, so it is not repeated here.
      typecheck: { command: 'vue-tsc --noEmit -p tsconfig.json' },
      build: { command: 'tauri build', cache: false, dependsOn: ['typecheck'] },
      verify: { command: 'node tools/scripts/verify-prod-bundle.mjs', cache: false },
      // Reads GITHUB_REF_NAME when there is one, and the answer depends on files a cache key would
      // not cover if it did not, so this is never cached.
      versions: { command: 'node tools/scripts/check-versions.mjs', cache: false },
      // Reads target/, which is gitignored and outside any cache key, so a hit would report on a
      // build it never looked at.
      bundles: { command: 'node tools/scripts/check-bundles.mjs', cache: false },
      // Launches the built binary. cache:false for the same reason `dev` is: the point is the side
      // effect, and a replayed stdout would report a launch that never happened.
      smoke: { command: 'node tools/scripts/smoke.mjs', cache: false },
      types: { command: 'cargo test -p repo-scan --features typescript', cache: false },
      rust: {
        command: 'cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test',
        cache: false,
      },
    },
  },
});
