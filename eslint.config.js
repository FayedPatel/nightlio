import js from '@eslint/js'
import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import jsxA11y from 'eslint-plugin-jsx-a11y'
import tseslint from 'typescript-eslint'
import { defineConfig, globalIgnores } from 'eslint/config'

// jsx-a11y runs at the recommended preset's full 'error' severity: the
// accessibility backlog that once forced a blanket downgrade to 'warn' has
// been cleared, so regressions now fail lint instead of accumulating.

export default defineConfig([
  globalIgnores(['dist', 'node_modules', 'api/venv/**', '**/site-packages/**', '.claude/**', 'coverage', 'playwright-report', 'test-results', 'screenshots']),
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
      'react-refresh/only-export-components': 'off',
    },
  },
  {
    // TypeScript sources. typescript-eslint recommended (not the type-checked
    // variant — that ratchet comes at the end of the migration), with the same
    // react-hooks/react-refresh/jsx-a11y coverage the js/jsx block gets, so a
    // file renamed .jsx -> .tsx never silently drops out of linting.
    files: ['**/*.{ts,tsx}'],
    extends: [
      tseslint.configs.recommended,
      reactHooks.configs['recommended-latest'],
      reactRefresh.configs.vite,
      jsxA11y.flatConfigs.recommended,
    ],
    languageOptions: {
      globals: globals.browser,
    },
    rules: {
      'react-refresh/only-export-components': 'off',
    },
  },
  {
    files: ['*.config.{js,ts}'],
    languageOptions: {
      globals: { ...globals.node },
    },
  },
  {
    files: ['src/**/*.test.{js,jsx,ts,tsx}', 'src/test/**', 'e2e/**'],
    languageOptions: {
      globals: { ...globals.browser, ...globals.node, ...globals.vitest },
    },
  },
])
