const { defineConfig, devices } = require('../../web/node_modules/@playwright/test');

const fixtureApiKey = process.env.NOCTIS_E2E_API_KEY;
if (!fixtureApiKey) throw new Error('NOCTIS_E2E_API_KEY is required for E2E');

module.exports = defineConfig({
  testDir: '.',
  timeout: 180_000,
  outputDir: '../../web/test-results',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: 'line',
  use: {
    baseURL: 'http://127.0.0.1:4173',
    trace: 'retain-on-failure',
    viewport: { width: 1440, height: 900 }
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] }
    }
  ],
  webServer: {
    command: 'node tests/e2e/runtime.cjs',
    cwd: '../..',
    url: 'http://127.0.0.1:4173',
    reuseExistingServer: false,
    timeout: 180_000,
    env: { ...process.env, NOCTIS_E2E_API_KEY: fixtureApiKey }
  }
});
