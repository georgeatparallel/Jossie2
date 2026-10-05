-- Older work-plan schemas admitted task-status vocabulary as a goal status.
-- Recover those goals so continuation, controls, and workspace queries see them.
UPDATE goals SET status = 'active' WHERE status = 'in_progress';

CREATE TRIGGER goals_validate_status_insert
BEFORE INSERT ON goals
WHEN NEW.status NOT IN ('active', 'blocked', 'paused', 'completed', 'cancelled')
BEGIN
    SELECT RAISE(ABORT, 'Unsupported goal status');
END;

CREATE TRIGGER goals_validate_status_update
BEFORE UPDATE OF status ON goals
WHEN NEW.status NOT IN ('active', 'blocked', 'paused', 'completed', 'cancelled')
BEGIN
    SELECT RAISE(ABORT, 'Unsupported goal status');
END;
