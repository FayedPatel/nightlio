import { defineConfig, devices } from '@playwright/test';

// E2E harness.
// - API: one instance PER WORKER, booted by the worker fixture in
//   e2e/support/fixtures.ts (scripts/e2e-api.sh with E2E_API_PORT set):
//   own port, own throwaway SQLite file, so workers can run in parallel
//   without trampling each other's data.
// - Frontend: one shared Vite dev server. Its /api proxy routes each
//   request to the right worker's backend via the x-e2e-api-port header
//   (see vite.config.ts). Dev server registers no service worker in dev
//   (devOptions.enabled is false); serviceWorkers: 'block' is belt and
//   braces on top of that.
export default defineConfig({
  testDir: 'e2e',
  // Spec files fan out across workers; tests inside a file stay ordered.
  fullyParallel: false,
  // Public-repo ubuntu-latest runners have 4 vCPUs — match them; each
  // worker's API process is cheap next to its chromium instance.
  workers: 4,
  retries: process.env.CI ? 1 : 0,
  // HTML report is written locally too (playwright-report/, gitignored) so
  // `yarn playwright show-report` works after any run, not just on CI.
  reporter: process.env.CI
    ? [['github'], ['html', { open: 'never' }]]
    : [['list'], ['html', { open: 'never' }]],
  use: {
    baseURL: 'http://localhost:5173',
    serviceWorkers: 'block',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  webServer: [
    // reuseExistingServer stays false: specs wipe entries/goals through the
    // API, so silently attaching to whatever already listens on this port
    // (docker compose, a dev server) would destroy real data. A busy port
    // must fail the run loudly instead.
    {
      // E2E=1 lets the vite e2e router answer un-stamped /api requests
      // itself instead of proxying them to the (dead) default target.
      command: 'E2E=1 yarn dev --port 5173 --strictPort',
      url: 'http://localhost:5173',
      reuseExistingServer: false,
      timeout: 60_000,
    },
  ],
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
      testIgnore: /mobile\.spec\.ts/,
    },
    {
      // Pixel 7 is Chromium-based; an iPhone preset would demand the WebKit
      // browser download. 412px viewport still exercises the <=640px layout.
      name: 'mobile',
      use: { ...devices['Pixel 7'] },
      testMatch: /mobile\.spec\.ts/,
    },
    {
      // Galaxy S25 Ultra: no Playwright preset, so a custom profile. Large
      // phones report a ~600-620px CSS viewport (see src/index.css — the
      // mobile breakpoint moved 600 -> 640 precisely because this band used
      // to fall into the tablet layout). Pixel 7's 412px never touches that
      // edge zone. This project runs the FULL feature suite so every feature
      // is verified on both desktop and the phone layout.
      name: 'mobile-large',
      use: {
        ...devices['Pixel 7'],
        viewport: { width: 620, height: 1340 },
        deviceScaleFactor: 3,
        userAgent:
          'Mozilla/5.0 (Linux; Android 15; SM-S938B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36',
      },
      // Header search is CSS-hidden at <=640px, so its spec can't run here.
      testIgnore: /search\.spec\.ts/,
    },
  ],
});
