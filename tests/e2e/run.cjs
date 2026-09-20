const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const web = process.cwd();
const cli = path.join(web, 'node_modules', '@playwright', 'test', 'cli.js');
const result = spawnSync(process.execPath, [cli, 'test', '--config', '../tests/e2e/playwright.config.cjs', ...process.argv.slice(2)], {
  cwd: web,
  env: process.env,
  stdio: 'inherit'
});

spawnSync('docker', ['rm', '--force', '--volumes', 'noctis-agent-3-e2e-postgres'], { stdio: 'ignore' });
fs.rmSync(path.join(web, 'test-results'), { recursive: true, force: true });
process.exit(result.status ?? 1);
