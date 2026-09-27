ALTER TABLE projects ADD CONSTRAINT projects_name_nonempty CHECK (btrim(name) <> '');
ALTER TABLE project_runs
    ADD COLUMN acceptance_criteria jsonb NOT NULL DEFAULT '[]',
    ADD COLUMN reserved_tokens bigint NOT NULL DEFAULT 0,
    ADD CONSTRAINT project_runs_criteria_array CHECK (jsonb_typeof(acceptance_criteria) = 'array'),
    ADD CONSTRAINT project_runs_budget_safe CHECK (token_budget <= 9007199254740991),
    ADD CONSTRAINT project_runs_reservation_check CHECK (reserved_tokens BETWEEN 0 AND token_budget);

CREATE TABLE plans (
    id text PRIMARY KEY CHECK (btrim(id) <> ''),
    project_run_id uuid NOT NULL REFERENCES project_runs(id) ON DELETE CASCADE,
    version bigint NOT NULL CHECK (version BETWEEN 1 AND 9007199254740991),
    tasks jsonb NOT NULL CHECK (jsonb_typeof(tasks) = 'array' AND jsonb_array_length(tasks) > 0),
    risk_flags jsonb NOT NULL DEFAULT '[]' CHECK (jsonb_typeof(risk_flags) = 'array'),
    status text NOT NULL DEFAULT 'PROPOSED' CHECK (status IN ('PROPOSED', 'APPROVED', 'REJECTED', 'SUPERSEDED')),
    actor_id text,
    reason text,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project_run_id, version)
);
CREATE UNIQUE INDEX plans_one_approved_per_run ON plans (project_run_id) WHERE status = 'APPROVED';

ALTER TABLE tasks ADD COLUMN plan_id text REFERENCES plans(id) ON DELETE RESTRICT;

-- ponytail: snapshot dan kontrak yang disetujui tetap beku; plan baru perlu task ID baru.
CREATE FUNCTION protect_approved_plan() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.status IN ('APPROVED', 'SUPERSEDED') THEN
        IF TG_OP = 'DELETE' OR OLD.status = 'SUPERSEDED' OR NEW.status <> 'SUPERSEDED'
           OR (NEW.id, NEW.project_run_id, NEW.version, NEW.tasks, NEW.risk_flags, NEW.actor_id, NEW.reason)
              IS DISTINCT FROM
              (OLD.id, OLD.project_run_id, OLD.version, OLD.tasks, OLD.risk_flags, OLD.actor_id, OLD.reason) THEN
            RAISE EXCEPTION 'approved plan is immutable' USING ERRCODE = '23514';
        END IF;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER approved_plan_guard BEFORE UPDATE OR DELETE ON plans
    FOR EACH ROW EXECUTE FUNCTION protect_approved_plan();

CREATE FUNCTION protect_approved_task() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF NEW.plan_id IS NOT NULL AND EXISTS (SELECT 1 FROM plans WHERE id = NEW.plan_id AND status <> 'PROPOSED') THEN
            RAISE EXCEPTION 'plan task insertion requires proposed plan' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;
    IF TG_OP = 'UPDATE' AND NEW.plan_id IS DISTINCT FROM OLD.plan_id
       AND NEW.plan_id IS NOT NULL
       AND EXISTS (SELECT 1 FROM plans WHERE id = NEW.plan_id AND status <> 'PROPOSED') THEN
        RAISE EXCEPTION 'plan task reassignment requires proposed plan' USING ERRCODE = '23514';
    END IF;
    IF EXISTS (SELECT 1 FROM plans WHERE id = OLD.plan_id AND status IN ('APPROVED', 'SUPERSEDED')) THEN
        IF TG_OP = 'DELETE' THEN
            RAISE EXCEPTION 'approved task is immutable' USING ERRCODE = '23514';
        END IF;
        IF (NEW.id, NEW.project_run_id, NEW.parent_id, NEW.role, NEW.title, NEW.objective,
            NEW.allowed_paths, NEW.acceptance_criteria, NEW.verification_commands,
            NEW.max_input_tokens, NEW.max_output_tokens, NEW.max_attempts, NEW.context_refs,
            NEW.max_tool_calls, NEW.timeout_seconds, NEW.plan_id)
           IS DISTINCT FROM
           (OLD.id, OLD.project_run_id, OLD.parent_id, OLD.role, OLD.title, OLD.objective,
            OLD.allowed_paths, OLD.acceptance_criteria, OLD.verification_commands,
            OLD.max_input_tokens, OLD.max_output_tokens, OLD.max_attempts, OLD.context_refs,
            OLD.max_tool_calls, OLD.timeout_seconds, OLD.plan_id) THEN
            RAISE EXCEPTION 'approved task contract is immutable' USING ERRCODE = '23514';
        END IF;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER approved_task_guard BEFORE INSERT OR UPDATE OR DELETE ON tasks
    FOR EACH ROW EXECUTE FUNCTION protect_approved_task();

CREATE FUNCTION protect_approved_dependency() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        IF EXISTS (SELECT 1 FROM tasks t JOIN plans p ON p.id = t.plan_id
                   WHERE t.id = NEW.task_id AND p.status <> 'PROPOSED') THEN
            RAISE EXCEPTION 'plan dependency insertion requires proposed plan' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;
    IF TG_OP = 'UPDATE' AND EXISTS (SELECT 1 FROM tasks t JOIN plans p ON p.id = t.plan_id
               WHERE t.id = NEW.task_id AND p.status <> 'PROPOSED') THEN
        RAISE EXCEPTION 'plan dependency reassignment requires proposed plan' USING ERRCODE = '23514';
    END IF;
    IF EXISTS (SELECT 1 FROM tasks t JOIN plans p ON p.id = t.plan_id
               WHERE t.id = OLD.task_id AND p.status IN ('APPROVED', 'SUPERSEDED')) THEN
        RAISE EXCEPTION 'approved dependency is immutable' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER approved_dependency_guard BEFORE INSERT OR UPDATE OR DELETE ON task_dependencies
    FOR EACH ROW EXECUTE FUNCTION protect_approved_dependency();
