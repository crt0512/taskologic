-- How many finished tasks of a step a program wants before the board shows
-- an estimate for that step. Was a fixed three; three stays the default.
ALTER TABLE programs ADD COLUMN min_samples INTEGER NOT NULL DEFAULT 3;
