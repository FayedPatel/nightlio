import { readFileSync } from 'node:fs';
import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

// Separate from vite.config.ts on purpose: the app config loads VitePWA
// (injectManifest), which would drag service-worker manifest generation and
// the virtual:pwa-register module into the test pipeline.
const pkg = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf-8'));

export default defineConfig({
  // Mirrors the __APP_VERSION__ define in vite.config.ts — without it,
  // component tests referencing __APP_VERSION__ throw at import time.
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  plugins: [react()],
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: './src/test/setup.ts',
    include: ['src/**/*.test.{js,jsx,ts,tsx}'],
    coverage: {
      provider: 'v8',
      include: ['src/**'],
      exclude: ['src/sw.ts', 'src/assets/**', 'src/test/**', 'src/**/*.test.*'],
    },
  },
});
