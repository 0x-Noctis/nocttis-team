# Sample Project Fixture

Small offline Git repository used by Noctis end-to-end tests. It contains a
dummy product backend and HTML frontend with five fixed task scenarios.

```sh
./reset.sh
./test.sh
```

`reset.sh` recreates `.fixture-repo` from `template`, initializes `main`, and
uses fixed Git metadata. Repeated resets produce the same commit SHA. `test.sh`
only uses Node.js built-ins and must pass before any scenario mutation.

Scenarios live in `scenarios/tasks.json`:

- `backend-only`: backend validation change;
- `frontend-only`: frontend markup change;
- `cross-stack`: backend and frontend contract change;
- `test-failure`: known bad mutation detected by tests;
- `file-conflict`: two tasks claim the same file.

Paths in scenario `allowed_paths` are relative to `.fixture-repo`. Scenario
commands assume this directory as the current working directory.
