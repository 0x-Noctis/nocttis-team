ALTER TABLE task_dependencies DROP CONSTRAINT task_dependencies_task_id_fkey;
ALTER TABLE task_dependencies DROP CONSTRAINT task_dependencies_dependency_id_fkey;
DO $$
DECLARE constraint_name text;
BEGIN
    FOR constraint_name IN
        SELECT conname FROM pg_constraint
        WHERE conrelid = 'task_dependencies'::regclass AND contype = 'c'
    LOOP
        EXECUTE format('ALTER TABLE task_dependencies DROP CONSTRAINT %I', constraint_name);
    END LOOP;
END $$;
ALTER TABLE agent_runs DROP CONSTRAINT agent_runs_task_id_fkey;
ALTER TABLE events DROP CONSTRAINT events_task_id_fkey;
ALTER TABLE file_leases DROP CONSTRAINT file_leases_task_id_fkey;
ALTER TABLE artifacts DROP CONSTRAINT artifacts_task_id_fkey;
ALTER TABLE tasks DROP CONSTRAINT tasks_parent_id_fkey;

ALTER TABLE tasks ALTER COLUMN id TYPE text USING id::text;
ALTER TABLE tasks ALTER COLUMN parent_id TYPE text USING parent_id::text;
ALTER TABLE task_dependencies ALTER COLUMN task_id TYPE text USING task_id::text;
ALTER TABLE task_dependencies ALTER COLUMN dependency_id TYPE text USING dependency_id::text;
ALTER TABLE agent_runs ALTER COLUMN task_id TYPE text USING task_id::text;
ALTER TABLE events ALTER COLUMN task_id TYPE text USING task_id::text;
ALTER TABLE file_leases ALTER COLUMN task_id TYPE text USING task_id::text;
ALTER TABLE artifacts ALTER COLUMN task_id TYPE text USING task_id::text;

DROP INDEX tasks_scheduler_idx;
ALTER TABLE tasks ALTER COLUMN status TYPE text USING status::text;
CREATE INDEX tasks_scheduler_idx ON tasks (status, priority DESC, created_at) WHERE status = 'READY';

ALTER TABLE tasks
    ADD CONSTRAINT tasks_id_nonempty CHECK (btrim(id) <> ''),
    ADD CONSTRAINT tasks_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES tasks(id) ON DELETE SET NULL,
    ADD COLUMN context_refs jsonb NOT NULL DEFAULT '[]',
    ADD COLUMN max_tool_calls bigint NOT NULL DEFAULT 1,
    ADD COLUMN timeout_seconds bigint NOT NULL DEFAULT 1,
    ADD COLUMN version bigint NOT NULL DEFAULT 0,
    ADD CONSTRAINT tasks_status_domain_check CHECK (status IN ('DRAFT','PLANNED','READY','ASSIGNED','RUNNING','SELF_CHECK','REVIEW','CHANGES_REQUESTED','VERIFY','FAILED','INTEGRATE','CONFLICT','NEEDS_HUMAN','DONE','CANCELLED')),
    ADD CONSTRAINT tasks_allowed_paths_array CHECK (jsonb_typeof(allowed_paths) = 'array'),
    ADD CONSTRAINT tasks_acceptance_criteria_array CHECK (jsonb_typeof(acceptance_criteria) = 'array'),
    ADD CONSTRAINT tasks_verification_commands_array CHECK (jsonb_typeof(verification_commands) = 'array'),
    ADD CONSTRAINT tasks_context_refs_array CHECK (jsonb_typeof(context_refs) = 'array'),
    ADD CONSTRAINT tasks_max_tool_calls_safe CHECK (max_tool_calls BETWEEN 1 AND 9007199254740991),
    ADD CONSTRAINT tasks_timeout_seconds_safe CHECK (timeout_seconds BETWEEN 1 AND 9007199254740991),
    ADD CONSTRAINT tasks_version_safe CHECK (version BETWEEN 0 AND 9007199254740991),
    ADD CONSTRAINT tasks_max_attempts_ceiling CHECK (max_attempts <= 10);

ALTER TABLE tasks ALTER COLUMN input_token_limit TYPE bigint;
ALTER TABLE tasks ALTER COLUMN output_token_limit TYPE bigint;
ALTER TABLE tasks RENAME COLUMN input_token_limit TO max_input_tokens;
ALTER TABLE tasks RENAME COLUMN output_token_limit TO max_output_tokens;
ALTER TABLE tasks
    ADD CONSTRAINT tasks_max_input_tokens_safe CHECK (max_input_tokens <= 9007199254740991),
    ADD CONSTRAINT tasks_max_output_tokens_safe CHECK (max_output_tokens <= 9007199254740991);

ALTER TABLE task_dependencies
    ADD CONSTRAINT task_dependencies_task_id_fkey FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE,
    ADD CONSTRAINT task_dependencies_dependency_id_fkey FOREIGN KEY (dependency_id) REFERENCES tasks(id) ON DELETE CASCADE,
    ADD CONSTRAINT task_dependencies_check CHECK (task_id <> dependency_id);

ALTER TABLE agent_runs
    ALTER COLUMN attempt TYPE bigint,
    ADD CONSTRAINT agent_runs_task_id_fkey FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE,
    ADD CONSTRAINT agent_runs_attempt_safe CHECK (attempt <= 9007199254740991);

ALTER TABLE events
    ADD CONSTRAINT events_task_id_fkey FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE,
    ADD COLUMN from_status text,
    ADD COLUMN to_status text,
    ADD CONSTRAINT events_actor_type_check CHECK (actor_type IN ('worker', 'reviewer', 'verifier', 'integrator', 'human', 'system')),
    ADD CONSTRAINT events_from_status_domain_check CHECK (from_status IS NULL OR from_status IN ('DRAFT','PLANNED','READY','ASSIGNED','RUNNING','SELF_CHECK','REVIEW','CHANGES_REQUESTED','VERIFY','FAILED','INTEGRATE','CONFLICT','NEEDS_HUMAN','DONE','CANCELLED')),
    ADD CONSTRAINT events_to_status_domain_check CHECK (to_status IS NULL OR to_status IN ('DRAFT','PLANNED','READY','ASSIGNED','RUNNING','SELF_CHECK','REVIEW','CHANGES_REQUESTED','VERIFY','FAILED','INTEGRATE','CONFLICT','NEEDS_HUMAN','DONE','CANCELLED')),
    ADD CONSTRAINT events_transition_shape_check CHECK (
        (event_type = 'status_transition' AND task_id IS NOT NULL AND from_status IS NOT NULL AND to_status IS NOT NULL)
        OR (event_type <> 'status_transition' AND from_status IS NULL AND to_status IS NULL)
    ),
    ADD CONSTRAINT events_payload_object_check CHECK (jsonb_typeof(payload) = 'object');

ALTER TABLE file_leases
    ADD CONSTRAINT file_leases_task_id_fkey FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE;
ALTER TABLE artifacts
    ADD CONSTRAINT artifacts_task_id_fkey FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE;

ALTER TABLE model_usage
    ALTER COLUMN input_tokens TYPE bigint,
    ALTER COLUMN cached_tokens TYPE bigint,
    ALTER COLUMN output_tokens TYPE bigint,
    ALTER COLUMN latency_ms TYPE bigint,
    ADD COLUMN tool_calls bigint NOT NULL DEFAULT 0,
    ADD CONSTRAINT model_usage_input_tokens_safe CHECK (input_tokens <= 9007199254740991),
    ADD CONSTRAINT model_usage_cached_tokens_safe CHECK (cached_tokens <= 9007199254740991),
    ADD CONSTRAINT model_usage_output_tokens_safe CHECK (output_tokens <= 9007199254740991),
    ADD CONSTRAINT model_usage_latency_ms_safe CHECK (latency_ms <= 9007199254740991),
    ADD CONSTRAINT model_usage_tool_calls_safe CHECK (tool_calls BETWEEN 0 AND 9007199254740991);
