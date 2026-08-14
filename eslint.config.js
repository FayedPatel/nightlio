import js from '@eslint/js'
import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import jsxA11y from 'eslint-plugin-jsx-a11y'
import { defineConfig, globalIgnores } from 'eslint/config'

export default defineConfig([
  globalIgnores(['dist', 'node_modules', 'api/venv/**', '**/site-packages/**', '.claude/**']),
  {
    files: ['**/*.{js,jsx}'],
    extends: [
      js.configs.recommended,
      reactHooks.configs['recommended-latest'],
      reactRefresh.configs.vite,
      jsxA11y.flatConfigs.recommended,
    ],
    languageOptions: {
      ecmaVersion: 2020,
      globals: globals.browser,
      parserOptions: {
        ecmaVersion: 'latest',
        ecmaFeatures: { jsx: true },
        sourceType: 'module',
      },
    },
    rules: {
      // argsIgnorePattern mirrors varsIgnorePattern: without eslint-plugin-react's
      // jsx-uses-vars rule, core no-unused-vars cannot see JSX references, so
      // capitalized component identifiers (e.g. a destructured `icon: Icon` param
      // rendered as <Icon />) must be exempted in both positions.
      'no-unused-vars': ['error', { varsIgnorePattern: '^[A-Z_]', argsIgnorePattern: '^[A-Z_]' }],
      'react-refresh/only-export-components': 'off',
      // jsx-a11y adopted incrementally: existing components have an accessibility
      // backlog (see PLAN.md Phase 4). Findings are reported, not mass-fixed, so
      // every rule the recommended preset turns on as 'error' is downgraded to
      // 'warn' here to avoid blocking CI while components are fixed over time.
      // Rules the preset leaves 'off' stay off. Re-promote to 'error' once the
      // backlog (see hardening-agent report) is clear.
      ...Object.fromEntries(
        Object.entries(jsxA11y.flatConfigs.recommended.rules)
          .filter(([, severity]) => severity === 'error' || severity[0] === 'error')
          .map(([rule, severity]) =>
            Array.isArray(severity) ? [rule, ['warn', ...severity.slice(1)]] : [rule, 'warn'],
          ),
      ),
    },
  },
  {
    files: ['vite.config.js', 'vitest.config.js', 'playwright.config.js'],
    languageOptions: {
      globals: { ...globals.node },
    },
  },
  {
    files: ['src/**/*.test.{js,jsx}', 'src/test/**', 'e2e/**'],
    languageOptions: {
      globals: { ...globals.browser, ...globals.node, ...globals.vitest },
    },
  },
])
