CREATE TYPE task_status AS ENUM (
    'DRAFT', 'PLANNED', 'READY', 'ASSIGNED', 'RUNNING', 'SELF_CHECK',
    'REVIEW', 'CHANGES_REQUESTED', 'VERIFY', 'FAILED', 'INTEGRATE',
    'CONFLICT', 'NEEDS_HUMAN', 'DONE', 'CANCELLED', 'FAILED_FINAL'
);

CREATE TABLE projects (
    id uuid PRIMARY KEY,
    name text NOT NULL,
    repository_path text NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE project_runs (
    id uuid PRIMARY KEY,
    project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    objective text NOT NULL,
    status text NOT NULL,
    token_budget bigint NOT NULL CHECK (token_budget > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE tasks (
    id uuid PRIMARY KEY,
    project_run_id uuid NOT NULL REFERENCES project_runs(id) ON DELETE CASCADE,
    parent_id uuid REFERENCES tasks(id),
    role text NOT NULL,
    title text NOT NULL,
    objective text NOT NULL,
    status task_status NOT NULL DEFAULT 'DRAFT',
    allowed_paths jsonb NOT NULL DEFAULT '[]',
    acceptance_criteria jsonb NOT NULL DEFAULT '[]',
    verification_commands jsonb NOT NULL DEFAULT '[]',
    input_token_limit integer NOT NULL CHECK (input_token_limit > 0),
    output_token_limit integer NOT NULL CHECK (output_token_limit > 0),
    max_attempts smallint NOT NULL DEFAULT 2 CHECK (max_attempts > 0),
    priority integer NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX tasks_scheduler_idx
    ON tasks (status, priority DESC, created_at)
    WHERE status = 'READY';

CREATE TABLE task_dependencies (
    task_id uuid NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    dependency_id uuid NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, dependency_id),
    CHECK (task_id <> dependency_id)
);

CREATE TABLE agent_runs (
    id uuid PRIMARY KEY,
    task_id uuid NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    role text NOT NULL,
    provider_id text NOT NULL,
    model_id text NOT NULL,
    attempt smallint NOT NULL CHECK (attempt > 0),
    status text NOT NULL,
    worktree_path text,
    base_commit text,
    heartbeat_at timestamptz,
    started_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    error_code text,
    UNIQUE (task_id, attempt)
);

CREATE TABLE events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    project_run_id uuid NOT NULL REFERENCES project_runs(id) ON DELETE CASCADE,
    task_id uuid REFERENCES tasks(id) ON DELETE CASCADE,
    actor_type text NOT NULL,
    actor_id text,
    event_type text NOT NULL,
    payload jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX events_run_cursor_idx ON events (project_run_id, id);

CREATE TABLE model_usage (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    agent_run_id uuid NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    request_id text,
    input_tokens integer NOT NULL CHECK (input_tokens >= 0),
    cached_tokens integer NOT NULL DEFAULT 0 CHECK (cached_tokens >= 0),
    output_tokens integer NOT NULL CHECK (output_tokens >= 0),
    estimated boolean NOT NULL DEFAULT false,
    cost_micros bigint CHECK (cost_micros >= 0),
    latency_ms integer NOT NULL CHECK (latency_ms >= 0),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE file_leases (
    project_run_id uuid NOT NULL REFERENCES project_runs(id) ON DELETE CASCADE,
    path_pattern text NOT NULL,
    task_id uuid NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (project_run_id, path_pattern)
);

CREATE TABLE artifacts (
    id uuid PRIMARY KEY,
    task_id uuid NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    kind text NOT NULL,
    path text NOT NULL,
    size_bytes bigint NOT NULL CHECK (size_bytes >= 0),
    sha256 text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

