# Browser smoke test

## Prerequisites

- Node.js version supported by `web/package.json` dependencies.
- Rust backend dependencies and PostgreSQL when testing API-backed flows.
- Backend URL: `http://127.0.0.1:7410`.
- Frontend URL: `http://127.0.0.1:4173`; Playwright starts Vite automatically.
- Playwright-managed Chromium. Do not use system Firefox.

## Install

```bash
cd web
npm ci
npx playwright install chromium
```

Linux hosts missing browser libraries may use the environment-approved equivalent of `npx playwright install --with-deps chromium`.

## Run foundation smoke

```bash
cd web
npm run check
npm run build
npm run e2e
```

Current automated smoke uses a 1440 × 900 desktop viewport. It verifies keyboard navigation from the home page to Providers, then keyboard activation of the primary Create task control on `/tasks`. It does not fake backend responses or claim the full provider-to-result vertical flow.

## Full manual vertical flow

1. Start PostgreSQL and backend on `127.0.0.1:7410`.
2. Start WebApp with `cd web && npm run dev -- --port 4173 --strictPort`.
3. Confirm a configured provider appears under `/providers` without exposing its API key.
4. Open `/tasks`, create a manual task, open its detail, and start it.
5. Observe live progress, including reconnect behavior after briefly disabling and restoring the browser network connection.
6. Confirm diff, artifacts, final result, and any error remain visible on task detail.
7. Repeat primary navigation and actions using `Tab`, `Shift+Tab`, `Enter`, and `Space` at a desktop viewport.

M2-015A installs runner foundation only. Full vertical-flow automation waits for integrated orchestration and must use real backend behavior.

## Cleanup

Stop backend and any manually started Vite process with `Ctrl+C`. Playwright stops its managed Vite process automatically. Remove retained test output with:

```bash
rm -rf web/test-results
```
