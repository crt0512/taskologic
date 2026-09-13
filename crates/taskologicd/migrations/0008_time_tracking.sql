-- Time tracking, 0.1.11.
--
-- Everything here either adds a column with a default or rebuilds one small
-- bookkeeping table, so `make update` stops the daemon, runs this and starts
-- it again with nothing left to convert by hand.

-- When work on a task is meant to begin, alongside when it is due. A task may
-- carry either date, both, or neither.
ALTER TABLE tasks ADD COLUMN start_at INTEGER;

-- The per task reminder override splits in two, one per date. The column that
-- exists always counted back from the due date, so it is renamed rather than
-- replaced and every task keeps the override it already had.
ALTER TABLE tasks RENAME COLUMN reminder_minutes TO reminder_due_minutes;
ALTER TABLE tasks ADD COLUMN reminder_start_minutes INTEGER;

-- Which template stamped this task out, so analytics can average the tasks
-- that came from the same one. Tasks made before this migration have no
-- answer and never will, so per template history starts here.
--
-- No foreign key on purpose: deleting a template must not delete or blank the
-- tasks it produced, the same reasoning the events table is built on.
ALTER TABLE tasks ADD COLUMN template_id INTEGER;
CREATE INDEX tasks_template ON tasks(template_id) WHERE template_id IS NOT NULL;

-- Set by hand on a task whose timing should not colour the averages.
ALTER TABLE tasks ADD COLUMN exclude_from_stats INTEGER NOT NULL DEFAULT 0;

-- A task with both dates now owes two reminders, and they must not dedupe
-- against each other. The old key was (task, uid, due_at), which cannot grow
-- a column in place, so the table is rebuilt.
--
-- `anchor_at` is whichever date the reminder counted back from. Keeping it in
-- the key is what makes moving a due date send a fresh reminder rather than
-- silently swallowing it. Every row that already exists was a due reminder.
CREATE TABLE reminders_sent_new (
    task_id   INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    uid       INTEGER NOT NULL,
    kind      TEXT    NOT NULL,
    anchor_at INTEGER NOT NULL,
    sent_at   INTEGER NOT NULL,
    PRIMARY KEY (task_id, uid, kind, anchor_at)
);

INSERT INTO reminders_sent_new (task_id, uid, kind, anchor_at, sent_at)
    SELECT task_id, uid, 'due', due_at, sent_at FROM reminders_sent;

DROP TABLE reminders_sent;
ALTER TABLE reminders_sent_new RENAME TO reminders_sent;
