ALTER TABLE agent_runs
    ADD COLUMN dispatch_owner uuid,
    ADD COLUMN dispatch_claimed_at timestamptz,
    ADD CONSTRAINT agent_runs_dispatch_owner_shape CHECK (
        (dispatch_owner IS NULL AND dispatch_claimed_at IS NULL)
        OR (dispatch_owner IS NOT NULL AND dispatch_claimed_at IS NOT NULL)
    );

CREATE INDEX agent_runs_dispatchable_idx
    ON agent_runs (started_at, id)
    WHERE status = 'assigned' AND finished_at IS NULL AND dispatch_owner IS NULL;
