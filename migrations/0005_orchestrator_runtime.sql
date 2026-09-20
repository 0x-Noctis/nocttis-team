ALTER TABLE agent_runs
    DROP COLUMN worktree_path,
    ADD COLUMN branch text,
    ADD COLUMN retain_until timestamptz,
    ADD CONSTRAINT agent_runs_branch_nonempty CHECK (branch IS NULL OR btrim(branch) <> ''),
    ADD CONSTRAINT agent_runs_base_commit_nonempty CHECK (base_commit IS NULL OR btrim(base_commit) <> ''),
    ADD CONSTRAINT agent_runs_runtime_status CHECK (
        branch IS NULL OR status IN ('running','completed','failed')
    ),
    ADD CONSTRAINT agent_runs_error_code_safe CHECK (
        error_code IS NULL OR (length(error_code) BETWEEN 1 AND 64 AND error_code ~ '^[A-Za-z0-9_.-]+$')
    ),
    ADD CONSTRAINT agent_runs_finished_shape CHECK (finished_at IS NULL OR finished_at >= started_at),
    ADD CONSTRAINT agent_runs_retention_shape CHECK (retain_until IS NULL OR retain_until >= started_at);

CREATE INDEX agent_runs_unfinished_idx
    ON agent_runs (heartbeat_at, started_at, id)
    WHERE finished_at IS NULL;

CREATE TABLE tool_call_reservations (
    agent_run_id uuid NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    call_id text NOT NULL CHECK (length(call_id) BETWEEN 1 AND 255 AND call_id !~ '[[:cntrl:]]'),
    status text NOT NULL DEFAULT 'in_progress' CHECK (status IN ('in_progress','completed')),
    outcome text CHECK (outcome IS NULL OR outcome IN ('succeeded','failed','timed_out')),
    duration_ms bigint CHECK (duration_ms BETWEEN 0 AND 9007199254740991),
    artifact_id uuid,
    created_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    PRIMARY KEY (agent_run_id, call_id),
    CHECK (
        (status = 'in_progress' AND outcome IS NULL AND duration_ms IS NULL AND artifact_id IS NULL AND completed_at IS NULL)
        OR (status = 'completed' AND outcome IS NOT NULL AND duration_ms IS NOT NULL AND completed_at IS NOT NULL)
    )
);

ALTER TABLE model_usage ADD COLUMN operation_key text;
ALTER TABLE model_usage
    ADD CONSTRAINT model_usage_operation_key_safe CHECK (
        operation_key IS NULL OR (length(operation_key) BETWEEN 1 AND 255 AND operation_key !~ '[[:cntrl:]]')
    );
CREATE UNIQUE INDEX model_usage_agent_operation_key
    ON model_usage (agent_run_id, operation_key)
    WHERE operation_key IS NOT NULL;

ALTER TABLE artifacts ALTER COLUMN path DROP NOT NULL;
ALTER TABLE artifacts
    ADD COLUMN logical_name text,
    ADD COLUMN media_type text,
    ADD CONSTRAINT artifacts_logical_name_safe CHECK (logical_name IS NULL OR btrim(logical_name) <> ''),
    ADD CONSTRAINT artifacts_media_type_safe CHECK (media_type IS NULL OR btrim(media_type) <> '');
