-- Deleting a task moves it to the archive; the row goes for good only after
-- the board's retention period or when the owner purges it.
ALTER TABLE tasks ADD COLUMN deleted_at INTEGER;
CREATE INDEX tasks_deleted ON tasks(deleted_at) WHERE deleted_at IS NOT NULL;
ALTER TABLE boards ADD COLUMN purge_deleted_after_secs INTEGER NOT NULL DEFAULT 2592000;
