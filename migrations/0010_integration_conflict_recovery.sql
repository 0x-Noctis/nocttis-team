DROP INDEX integration_operations_recovery_idx;
CREATE INDEX integration_operations_recovery_idx
    ON integration_operations (status, updated_at)
    WHERE status IN ('prepared','applied','conflict');
