ALTER TABLE events ADD COLUMN operation_key text;
CREATE UNIQUE INDEX events_task_operation_key_uq
    ON events (task_id, operation_key)
    WHERE operation_key IS NOT NULL;

CREATE TABLE integration_operations (
    id uuid PRIMARY KEY,
    attempt_id uuid NOT NULL UNIQUE REFERENCES agent_runs(id) ON DELETE CASCADE,
    task_id text NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    source_branch text NOT NULL,
    source_base_commit text NOT NULL,
    target_id text NOT NULL,
    target_branch text NOT NULL,
    target_base_commit text NOT NULL,
    patch_sha256 text NOT NULL CHECK (patch_sha256 ~ '^[0-9a-f]{64}$'),
    owner_token uuid NOT NULL,
    status text NOT NULL CHECK (status IN ('prepared','applied','completed','conflict')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX integration_operations_recovery_idx
    ON integration_operations (status, updated_at)
    WHERE status IN ('prepared','applied');
