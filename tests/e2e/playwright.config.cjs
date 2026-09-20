const { defineConfig, devices } = require('../../web/node_modules/@playwright/test');

module.exports = defineConfig({
  testDir: '.',
  outputDir: '../../web/test-results',
  fullyParallel: false,
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
    command: 'npm run dev -- --port 4173 --strictPort',
    cwd: '../../web',
    url: 'http://127.0.0.1:4173',
    reuseExistingServer: true,
    timeout: 120_000
  }
});
