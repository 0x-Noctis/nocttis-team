const { defineConfig, devices } = require('../../web/node_modules/@playwright/test');

const vertical = process.env.NOCTIS_VERTICAL_E2E === '1';
const fixtureApiKey = process.env.NOCTIS_E2E_API_KEY;
if (vertical && !fixtureApiKey) throw new Error('NOCTIS_E2E_API_KEY is required for vertical E2E');
const webServer = [
  {
    command: `npm run dev -- --port ${vertical ? '4174' : '4173'} --strictPort`,
    cwd: '../../web',
    url: `http://127.0.0.1:${vertical ? '4174' : '4173'}`,
    reuseExistingServer: true,
    timeout: 120_000
  }
];

if (vertical) {
  webServer.unshift(
    {
      command: 'node tests/e2e/fake-provider.cjs',
      cwd: '../..',
      url: 'http://127.0.0.1:7411/health',
      reuseExistingServer: false,
      timeout: 30_000
    },
    {
      command: 'cargo run --bin ai-team',
      cwd: '../..',
      url: 'http://127.0.0.1:7410/api/v1/health',
      reuseExistingServer: false,
      timeout: 120_000,
      env: {
        ...process.env,
        PRIMARY_API_KEY: fixtureApiKey,
        NOCTIS__SERVER__BIND: '127.0.0.1:7410',
        NOCTIS__PROVIDER__BASE_URL: 'http://127.0.0.1:7411/v1',
        NOCTIS__PROVIDER__MODEL: 'fixture-model'
      }
    },
    {
      command: 'node tests/e2e/gateway.cjs',
      cwd: '../..',
      url: 'http://127.0.0.1:4173',
      reuseExistingServer: false,
      timeout: 30_000
    }
  );
}

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
  webServer
});
