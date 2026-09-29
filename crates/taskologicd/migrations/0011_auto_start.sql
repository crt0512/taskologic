-- Auto start, 0.1.12. A task can ask to be moved into the started column
-- by the scheduler when its start date arrives, instead of waiting for a
-- person. Off for every task that exists, which is what they all did.
ALTER TABLE tasks ADD COLUMN auto_start INTEGER NOT NULL DEFAULT 0;
