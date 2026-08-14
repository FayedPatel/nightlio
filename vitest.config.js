import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

// Separate from vite.config.js on purpose: the app config loads VitePWA
// (injectManifest), which would drag service-worker manifest generation and
// the virtual:pwa-register module into the test pipeline.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: './src/test/setup.js',
    include: ['src/**/*.test.{js,jsx}'],
    coverage: {
      provider: 'v8',
      include: ['src/**'],
      exclude: ['src/sw.js', 'src/assets/**', 'src/test/**', 'src/**/*.test.*'],
    },
  },
});
