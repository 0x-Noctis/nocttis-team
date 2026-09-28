CREATE OR REPLACE FUNCTION protect_approved_task() RETURNS trigger LANGUAGE plpgsql AS $$
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
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END $$;

CREATE OR REPLACE FUNCTION protect_approved_dependency() RETURNS trigger LANGUAGE plpgsql AS $$
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
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END $$;
