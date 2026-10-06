-- M4-003: reservasi token per request.
-- Usage yang sudah terjadi tetap di model_usage. Tabel ini hanya menyimpan token yang DITAHAN untuk request
-- yang sedang berjalan, supaya beberapa attempt paralel dalam satu run tidak sama-sama lolos cek budget
-- (oversubscribe). Saat request selesai, reservasi dikonversi menjadi baris model_usage dalam satu transaksi.
CREATE TABLE budget_reservations (
    id uuid PRIMARY KEY,
    project_run_id uuid NOT NULL REFERENCES project_runs(id) ON DELETE CASCADE,
    task_id text NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    agent_run_id uuid NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    request_key text NOT NULL CHECK (btrim(request_key) <> ''),
    input_tokens bigint NOT NULL CHECK (input_tokens BETWEEN 0 AND 9007199254740991),
    output_tokens bigint NOT NULL CHECK (output_tokens BETWEEN 0 AND 9007199254740991),
    purpose text NOT NULL CHECK (purpose IN ('work', 'recovery')),
    status text NOT NULL DEFAULT 'held' CHECK (status IN ('held', 'settled', 'released')),
    created_at timestamptz NOT NULL DEFAULT now(),
    closed_at timestamptz,
    CONSTRAINT budget_reservations_closed_shape CHECK ((status = 'held') = (closed_at IS NULL)),
    UNIQUE (agent_run_id, request_key)
);

CREATE INDEX budget_reservations_held_run_idx ON budget_reservations (project_run_id) WHERE status = 'held';
CREATE INDEX budget_reservations_held_attempt_idx ON budget_reservations (agent_run_id) WHERE status = 'held';
