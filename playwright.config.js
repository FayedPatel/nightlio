import { defineConfig, devices } from '@playwright/test';

// E2E harness: Playwright boots both servers itself.
// - API: scripts/e2e-api.sh (fresh SQLite DB, OIDC off -> auto-login).
//   Health-checked via the unauthenticated GET /api/config.
// - Frontend: the Vite dev server, because it has the /api proxy (preview
//   does not) and registers no service worker in dev (devOptions.enabled is
//   false), so the SW controllerchange reload can't disturb tests.
//   serviceWorkers: 'block' is belt and braces on top of that.
export default defineConfig({
  testDir: 'e2e',
  // Single shared API database — parallel specs would trample each other.
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [['github'], ['html', { open: 'never' }]] : 'list',
  use: {
    baseURL: 'http://localhost:5173',
    serviceWorkers: 'block',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  webServer: [
    {
      command: './scripts/e2e-api.sh',
      url: 'http://localhost:5000/api/config',
      reuseExistingServer: !process.env.CI,
      timeout: 60_000,
    },
    {
      command: 'yarn dev --port 5173 --strictPort',
      url: 'http://localhost:5173',
      reuseExistingServer: !process.env.CI,
      timeout: 60_000,
    },
  ],
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
      testIgnore: /mobile\.spec\.js/,
    },
    {
      // Pixel 7 is Chromium-based; an iPhone preset would demand the WebKit
      // browser download. 412px viewport still exercises the <=640px layout.
      name: 'mobile',
      use: { ...devices['Pixel 7'] },
      testMatch: /mobile\.spec\.js/,
    },
    {
      // Galaxy S25 Ultra: no Playwright preset, so a custom profile. Large
      // phones report a ~600-620px CSS viewport (see src/index.css — the
      // mobile breakpoint moved 600 -> 640 precisely because this band used
      // to fall into the tablet layout). Pixel 7's 412px never touches that
      // edge zone, so this project guards the 600-640 band specifically.
      name: 'mobile-large',
      use: {
        ...devices['Pixel 7'],
        viewport: { width: 620, height: 1340 },
        deviceScaleFactor: 3,
        userAgent:
          'Mozilla/5.0 (Linux; Android 15; SM-S938B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36',
      },
      testMatch: /mobile\.spec\.js/,
    },
  ],
});
