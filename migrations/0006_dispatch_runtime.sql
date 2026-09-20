ALTER TABLE agent_runs DROP CONSTRAINT agent_runs_runtime_status;
ALTER TABLE agent_runs
    ADD COLUMN cleanup_completed_at timestamptz,
    ADD CONSTRAINT agent_runs_runtime_status CHECK (
        branch IS NULL OR status IN ('assigned','running','recovery_required','completed','failed')
    ),
    ADD CONSTRAINT agent_runs_cleanup_shape CHECK (
        cleanup_completed_at IS NULL OR finished_at IS NOT NULL
    );

CREATE INDEX agent_runs_stale_idx
    ON agent_runs (heartbeat_at, started_at, id)
    WHERE finished_at IS NULL AND status IN ('assigned','running');

CREATE INDEX agent_runs_retention_due_idx
    ON agent_runs (retain_until, id)
    WHERE finished_at IS NOT NULL AND cleanup_completed_at IS NULL;
