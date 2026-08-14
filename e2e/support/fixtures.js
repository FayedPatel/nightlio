// Per-worker backend isolation for parallel e2e runs.
//
// Every Playwright worker boots its own Flask API (own port, own throwaway
// SQLite file) via scripts/e2e-api.sh, and every browser request carries an
// x-e2e-api-port header that the vite dev proxy routes to that worker's
// backend (see vite.config.js). Specs import { test, expect } from here
// instead of '@playwright/test'; e2e/support/api.js picks the worker's port
// up from E2E_API_PORT, which is per-process because each worker is one.
import { spawn } from 'node:child_process';
import { test as base, expect } from '@playwright/test';

export const test = base.extend({
  apiPort: [
    // eslint-disable-next-line no-empty-pattern -- Playwright parses the destructuring to resolve fixture dependencies
    async ({}, run, workerInfo) => {
      const port = 5100 + workerInfo.workerIndex;
      process.env.E2E_API_PORT = String(port);
      const proc = spawn('./scripts/e2e-api.sh', {
        env: { ...process.env, E2E_API_PORT: String(port) },
        stdio: 'ignore',
        detached: true,
      });

      const deadline = Date.now() + 60_000;
      for (;;) {
        try {
          const resp = await fetch(`http://localhost:${port}/api/config`);
          if (resp.ok) break;
        } catch {
          // not up yet
        }
        if (Date.now() > deadline) {
          throw new Error(`e2e API on port ${port} never became healthy`);
        }
        await new Promise((resolve) => setTimeout(resolve, 250));
      }

      await run(port);

      // The script execs python directly, but kill the whole group in case
      // a future change reintroduces child processes.
      try {
        process.kill(-proc.pid, 'SIGTERM');
      } catch {
        try { proc.kill('SIGTERM'); } catch { /* already gone */ }
      }
    },
    { scope: 'worker', auto: true },
  ],

  context: async ({ context, apiPort }, run) => {
    await context.setExtraHTTPHeaders({ 'x-e2e-api-port': String(apiPort) });
    await run(context);
  },
});

export { expect };
