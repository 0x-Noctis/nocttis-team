# Single-Agent Baseline

Baseline M0-004 memakai lima skenario tetap dari fixture pada commit
`d064b65cd3941b73d80e95f4f3b66819bf87dd6c`. Setiap skenario dijalankan oleh
satu sesi agent pada salinan repository terisolasi. Tanggal pengukuran:
2026-09-19 (Asia/Jakarta).

Command:

```sh
codex exec --ephemeral --json --color never \
  --dangerously-bypass-approvals-and-sandbox \
  -C <isolated-fixture> <scenario-prompt>
node tests/baseline/verify.mjs
```

Model: `td/cx/gpt-5.6-sol-review` melalui provider `9router`, Codex CLI
`0.154.0`. Sandbox dilewati hanya karena setiap sesi berjalan dalam salinan
fixture disposable tanpa secret.

| Scenario | Success | Input tokens | Output tokens | Latency | Retry | Conflict | Tests | Human intervention | Cost |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| backend-only | yes | 43,706 | 1,350 | 24 s | 0 | 0 | 2/2 | 0 | N/A |
| frontend-only | yes | 62,833 | 1,417 | 29 s | 0 | 0 | 1/1 | 0 | N/A |
| cross-stack | yes | 61,118 | 2,072 | 53 s | 0 | 0 | 3/3 | 0 | N/A |
| test-failure | yes | 58,846 | 1,125 | 25 s | 0 | 0 | 1/1 | 0 | N/A |
| file-conflict | yes | 76,461 | 2,864 | 51 s | 0 | 1 expected | 3/3 | 0 | N/A |
| **Total / rate** | **100%** | **302,964** | **8,828** | **182 s** | **0** | **1 expected** | **10/10 (100%)** | **0** | **N/A** |

Mean per task: 60,592.8 input tokens, 1,765.6 output tokens, and 36.4 seconds.
Cost is unavailable: provider configuration and Codex JSON events exposed token
usage but no tariff. Applying an assumed public price would create false data.

Raw measured values, exact command template, fixture commit, changed paths, and
per-run notes are stored in `tests/baseline/results.json`. Original Codex event
streams are stored per scenario in `tests/baseline/*.jsonl`. Verification schema
and event token totals are enforced by `tests/baseline/verify.mjs`.

## Observations

- All five scenarios completed without human intervention or agent-level retry.
- Expected test failure blocked acceptance until the agent repaired the mutation.
- Expected overlapping price edits produced one Git conflict; the agent aborted it safely.
- Every final scenario verification passed; no secret appeared in captured output.
- Each Codex run warned that model metadata was unavailable and fallback metadata was used; token and latency results remain measured, but model tuning may differ from a registered model.
