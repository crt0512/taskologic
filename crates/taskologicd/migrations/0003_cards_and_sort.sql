-- What task cards show, per board, with a per user override in prefs.
ALTER TABLE boards ADD COLUMN card_fields_json TEXT NOT NULL DEFAULT '{}';
-- Columns can show their tasks by due date instead of the manual order.
ALTER TABLE columns ADD COLUMN sort_by_due INTEGER NOT NULL DEFAULT 0;
