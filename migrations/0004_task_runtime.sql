CREATE TABLE runtime_tasks (
    id text PRIMARY KEY CHECK (btrim(id) <> ''),
    project_id text NOT NULL CHECK (btrim(project_id) <> ''),
    title text NOT NULL CHECK (btrim(title) <> ''),
    role text NOT NULL CHECK (btrim(role) <> ''),
    objective text NOT NULL CHECK (btrim(objective) <> ''),
    status text NOT NULL CHECK (status IN (
        'DRAFT', 'PLANNED', 'READY', 'ASSIGNED', 'RUNNING', 'SELF_CHECK',
        'REVIEW', 'CHANGES_REQUESTED', 'VERIFY', 'FAILED', 'INTEGRATE',
        'CONFLICT', 'NEEDS_HUMAN', 'DONE', 'CANCELLED'
    )),
    depends_on jsonb NOT NULL,
    allowed_paths jsonb NOT NULL,
    context_refs jsonb NOT NULL,
    acceptance_criteria jsonb NOT NULL,
    verification_commands jsonb NOT NULL,
    max_input_tokens bigint NOT NULL CHECK (max_input_tokens BETWEEN 1 AND 9007199254740991),
    max_output_tokens bigint NOT NULL CHECK (max_output_tokens BETWEEN 1 AND 9007199254740991),
    max_tool_calls bigint NOT NULL CHECK (max_tool_calls BETWEEN 1 AND 9007199254740991),
    max_attempts smallint NOT NULL CHECK (max_attempts BETWEEN 1 AND 10),
    timeout_seconds bigint NOT NULL CHECK (timeout_seconds BETWEEN 1 AND 9007199254740991),
    version bigint NOT NULL DEFAULT 0 CHECK (version BETWEEN 0 AND 9007199254740991),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (jsonb_typeof(depends_on) = 'array'),
    CHECK (jsonb_typeof(allowed_paths) = 'array'),
    CHECK (jsonb_typeof(context_refs) = 'array'),
    CHECK (jsonb_typeof(acceptance_criteria) = 'array'),
    CHECK (jsonb_typeof(verification_commands) = 'array')
);

CREATE INDEX runtime_tasks_page_idx ON runtime_tasks (created_at, id);

CREATE TABLE task_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    task_id text NOT NULL REFERENCES runtime_tasks(id) ON DELETE CASCADE,
    actor text NOT NULL CHECK (actor IN ('worker', 'reviewer', 'verifier', 'integrator', 'human', 'system')),
    event_type text NOT NULL CHECK (btrim(event_type) <> ''),
    from_status text,
    to_status text,
    payload jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (jsonb_typeof(payload) = 'object')
);

CREATE INDEX task_events_task_cursor_idx ON task_events (task_id, id);

CREATE TABLE agent_attempts (
    id uuid PRIMARY KEY,
    task_id text NOT NULL REFERENCES runtime_tasks(id) ON DELETE CASCADE,
    attempt bigint NOT NULL CHECK (attempt BETWEEN 1 AND 9007199254740991),
    provider_id text NOT NULL CHECK (btrim(provider_id) <> ''),
    model_id text NOT NULL CHECK (btrim(model_id) <> ''),
    status text NOT NULL CHECK (btrim(status) <> ''),
    started_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    UNIQUE (task_id, attempt)
);

CREATE TABLE task_usage (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    attempt_id uuid NOT NULL REFERENCES agent_attempts(id) ON DELETE CASCADE,
    input_tokens bigint NOT NULL CHECK (input_tokens BETWEEN 0 AND 9007199254740991),
    cached_tokens bigint NOT NULL CHECK (cached_tokens BETWEEN 0 AND 9007199254740991),
    output_tokens bigint NOT NULL CHECK (output_tokens BETWEEN 0 AND 9007199254740991),
    tool_calls bigint NOT NULL CHECK (tool_calls BETWEEN 0 AND 9007199254740991),
    latency_ms bigint NOT NULL CHECK (latency_ms BETWEEN 0 AND 9007199254740991),
    created_at timestamptz NOT NULL DEFAULT now()
);
