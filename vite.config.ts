import { readFileSync } from 'node:fs'
import http from 'node:http'
import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'
import { VitePWA } from 'vite-plugin-pwa'

// Parallel e2e: each Playwright worker runs its own API instance and stamps
// its port on every browser request via an x-e2e-api-port header (see
// e2e/support/fixtures.js), so one dev server can front N isolated
// backends. Plain requests (normal dev) skip this middleware and use the
// standard /api proxy below. Implemented as a middleware because vite's
// proxy is raw http-proxy, which has no per-request router option.
const e2eApiRouter = (): Plugin => ({
  name: 'nightlio-e2e-api-router',
  configureServer(server) {
    server.middlewares.use((req, res, next) => {
      // req.url is always set on server requests; the guard only satisfies
      // @types/node's `string | undefined` and never fires at runtime.
      const url = req.url
      if (!url || !url.startsWith('/api')) return next()
      // Node lowercases incoming header names; a client sending the header
      // twice would surface string[], which never happens here — take the
      // first value so Number() sees the same input either way.
      const rawPort = req.headers['x-e2e-api-port']
      const port = Array.isArray(rawPort) ? rawPort[0] : rawPort
      if (!port) {
        // Under the e2e harness (E2E=1) nothing listens on the default
        // proxy target, and stray un-stamped requests exist (Chromium
        // fires an internal headerless GET after a fetch()-driven PDF
        // download). Answer them here instead of letting the internal
        // proxy error with ECONNREFUSED noise. Normal dev falls through.
        if (process.env.E2E) {
          res.statusCode = 404
          res.end('no x-e2e-api-port header')
          return
        }
        return next()
      }
      const upstream = http.request(
        {
          hostname: '127.0.0.1',
          port: Number(port),
          path: url,
          method: req.method,
          headers: { ...req.headers, host: `127.0.0.1:${port}` },
        },
        (upstreamRes) => {
          // statusCode is optional on IncomingMessage only because the type is
          // shared with client-side responses lacking one; an HTTP response
          // always carries it, so the fallback never fires at runtime.
          res.writeHead(upstreamRes.statusCode ?? 502, upstreamRes.headers)
          upstreamRes.pipe(res)
        },
      )
      upstream.on('error', () => {
        res.statusCode = 502
        res.end('e2e api unreachable')
      })
      req.pipe(upstream)
    })
  },
})

const pkg = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf-8'))

// Dark theme defaults from src/index.css (html[data-theme="dark"]): --bg: #282a36.
// The app's ThemeProvider defaults new users to dark mode, so the manifest/browser
// chrome colors follow that default.
const THEME_DARK_BG = '#282a36'

// https://vite.dev/config/
export default defineConfig({
  // Compile-time app version (mirrored in vitest.config.ts — keep in sync).
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  plugins: [
    e2eApiRouter(),
    react(),
    VitePWA({
      // We write the service worker by hand (src/sw.ts) so navigation requests can
      // be network-first with an explicit offline fallback, and /api/* can be a
      // hard NetworkOnly route — never cached, per product privacy constraint.
      strategies: 'injectManifest',
      srcDir: 'src',
      filename: 'sw.ts',
      injectManifest: {
        // Precache the built static assets + the offline fallback page.
        // /api/* is never part of the build output, so it can never end up here.
        globPatterns: ['**/*.{js,css,html,svg,png,ico,webmanifest}'],
        // index.html must NOT be precached: workbox's PrecacheRoute (registered
        // first in src/sw.ts) matches navigations to "/" via its default
        // directoryIndex and would serve the cached shell, silently overriding
        // the network-first navigation handler in src/sw.ts and pinning users
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
