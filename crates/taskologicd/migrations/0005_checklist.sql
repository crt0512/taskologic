-- Checklists on tasks. A small JSON array on the row, not its own table:
-- items have no identity outside their task and are read and written as one.
ALTER TABLE tasks ADD COLUMN checklist TEXT NOT NULL DEFAULT '[]';
