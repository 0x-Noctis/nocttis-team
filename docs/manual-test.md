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

## Run vertical smoke

The vertical test starts isolated PostgreSQL, the production Rust backend, a local OpenAI-compatible fixture, and a test-only same-origin gateway from `tests/e2e`. It never intercepts or mocks browser API routes. Docker, Git, Rust, Node.js, and Playwright Chromium must be available.

```bash
cd web
NOCTIS_E2E_API_KEY='<ephemeral fixture value>' \
npm run e2e -- --grep 'production vertical slice'
```

The test creates a provider and model against `http://127.0.0.1:7411/v1`, creates and starts a task using keyboard controls, reconnects through page reload and SSE backlog, rejects duplicate event IDs, and checks terminal status, verification, artifacts, diff, usage, persistent failure UI, and sensitive-output boundaries. Because no planning route currently promotes a manual task, the harness changes only its status from `DRAFT` to `READY` in isolated PostgreSQL; verification, usage, artifact, and diff data come from production orchestration and APIs.

## Full manual vertical flow

1. Start PostgreSQL and backend on `127.0.0.1:7410`.
2. Start WebApp with `cd web && npm run dev -- --port 4173 --strictPort`.
3. Confirm a configured provider appears under `/providers` without exposing its API key.
4. Open `/tasks`, create a manual task, open its detail, and start it.
5. Observe live progress, including reconnect behavior after briefly disabling and restoring the browser network connection.
6. Confirm diff, artifacts, final result, and any error remain visible on task detail.
7. Repeat primary navigation and actions using `Tab`, `Shift+Tab`, `Enter`, and `Space` at a desktop viewport.

M2-015A installs runner foundation. Vertical automation uses real backend behavior and is gated only while production orchestration is absent.

## Cleanup

Stop backend and any manually started Vite process with `Ctrl+C`. Playwright stops its managed Vite process automatically. Remove retained test output with:

```bash
rm -rf web/test-results
```

Playwright stops Vite, gateway, backend, fake-provider, and isolated PostgreSQL processes on exit. The wrapper removes test output and fixture storage on success or failure. If a run is interrupted, stop remaining listeners on ports `4173`, `4174`, `55432`, `7410`, and `7411` before retrying.
