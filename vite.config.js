import { readFileSync } from 'node:fs'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { VitePWA } from 'vite-plugin-pwa'

const pkg = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf-8'))

// Dark theme defaults from src/index.css (html[data-theme="dark"]): --bg: #282a36.
// The app's ThemeProvider defaults new users to dark mode, so the manifest/browser
// chrome colors follow that default.
const THEME_DARK_BG = '#282a36'

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    react(),
    VitePWA({
      // We write the service worker by hand (src/sw.js) so navigation requests can
      // be network-first with an explicit offline fallback, and /api/* can be a
      // hard NetworkOnly route — never cached, per product privacy constraint.
      strategies: 'injectManifest',
      srcDir: 'src',
      filename: 'sw.js',
      injectManifest: {
        // Precache the built static assets + the offline fallback page.
        // /api/* is never part of the build output, so it can never end up here.
        globPatterns: ['**/*.{js,css,html,svg,png,ico,webmanifest}'],
        // index.html must NOT be precached: workbox's PrecacheRoute (registered
        // first in src/sw.js) matches navigations to "/" via its default
        // directoryIndex and would serve the cached shell, silently overriding
        // the network-first navigation handler in src/sw.js and pinning users
        // to a stale build after every deploy. Excluding it lets navigations
        // genuinely hit the network first; offline.html (the offline fallback)
        // still matches the html glob above and stays precached.
        globIgnores: ['**/node_modules/**/*', '**/index.html'],
        // Pre-existing main bundle (MDXEditor + syntax highlighting, unrelated to
        // this phase) is ~2.3 MB unminified-for-cache; raise the default 2 MiB cap
        // so the app shell still precaches fully instead of silently dropping it.
        maximumFileSizeToCacheInBytes: 5 * 1024 * 1024,
      },
      registerType: 'autoUpdate',
      injectRegister: false, // registered manually in src/main.jsx via virtual:pwa-register
      includeAssets: ['logo.png', 'apple-touch-icon.png', 'offline.html'],
      manifest: {
        name: 'Nightlio',
        short_name: 'Nightlio',
        description: pkg.description,
        theme_color: THEME_DARK_BG,
        background_color: THEME_DARK_BG,
        display: 'standalone',
        start_url: '/',
        icons: [
          { src: 'pwa-192x192.png', sizes: '192x192', type: 'image/png', purpose: 'any' },
          { src: 'pwa-512x512.png', sizes: '512x512', type: 'image/png', purpose: 'any' },
          { src: 'maskable-icon-192x192.png', sizes: '192x192', type: 'image/png', purpose: 'maskable' },
          { src: 'maskable-icon-512x512.png', sizes: '512x512', type: 'image/png', purpose: 'maskable' },
        ],
      },
      devOptions: {
        enabled: false,
      },
    }),
  ],
  server: {
    host: true,
    proxy: {
      '/api': {
        target: (globalThis && globalThis.process && globalThis.process.env && globalThis.process.env.VITE_API_URL) || 'http://localhost:5000',
        changeOrigin: true,
      },
    },
  },
  preview: {
    host: true,
    port: 4173,
  },
})
