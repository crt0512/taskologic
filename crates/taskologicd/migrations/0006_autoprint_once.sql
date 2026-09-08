-- A task prints automatically once per person, not once per move.
--
-- Without this every move into the started column queues another slip, so
-- pausing and unpausing a task, or sending it back to the to-do column and
-- starting it again, prints it a second and a third time. A row here says
-- "this person has already had a slip for this task". A manual print is a
-- deliberate act and never consults it.
CREATE TABLE task_autoprint (
    task_id    INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    uid        INTEGER NOT NULL,
    printed_at INTEGER NOT NULL,
    PRIMARY KEY (task_id, uid)
);
