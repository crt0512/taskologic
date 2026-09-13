-- A task can override the user's default reminder lead time. Minutes, not
-- hours, so "half an hour before" is exact rather than rounded.
ALTER TABLE tasks ADD COLUMN reminder_minutes INTEGER;

-- Templates grew a due date prefill rule and dependencies on other
-- templates. One JSON column rather than three, because it is read and
-- written as a whole and the next option lands here without a migration.
ALTER TABLE templates ADD COLUMN options_json TEXT NOT NULL DEFAULT '{}';
