import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

// Separate from vite.config.ts on purpose: the app config loads VitePWA
// (injectManifest), which would drag service-worker manifest generation and
// the virtual:pwa-register module into the test pipeline.
export default defineConfig({
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
