-- Programs, 0.1.12: chains of tasks written once and started as often as
-- needed. See dev/PROGRAMS.md for the model. Three new tables and nothing
-- touched on the old ones, so `make update` runs this without a hand.

-- The definitions, per board like templates. The steps are one JSON array,
-- read and written as a whole; a step has no identity outside its program.
CREATE TABLE programs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    board_id    INTEGER NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
    owner_uid   INTEGER NOT NULL,
    name        TEXT    NOT NULL,
    description TEXT    NOT NULL DEFAULT '',
    steps_json  TEXT    NOT NULL
);
CREATE INDEX programs_board ON programs(board_id);

-- One started copy of a program. It keeps its own copy of the steps, so
-- editing or deleting the program later changes nothing about a run that is
-- already going. `column_id` is where the root was made and where every
-- step it makes lands too.
CREATE TABLE program_runs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    program_id   INTEGER REFERENCES programs(id) ON DELETE SET NULL,
    board_id     INTEGER NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
    root_task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    started_by   INTEGER NOT NULL,
    column_id    INTEGER NOT NULL,
    name         TEXT    NOT NULL,
    steps_json   TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    started_at   INTEGER,
    finished_at  INTEGER,
    cancelled_at INTEGER
);
CREATE INDEX program_runs_board ON program_runs(board_id, created_at);

-- Every task a run made, the root included, and what the run needs to
-- remember about it: which step and which go at it, whether the root waits
-- for it, its time limit, when it was paused, whether somebody deleted it
-- (skipped: its triggers never fire and the root stops waiting), the
-- question its step asks and where that stands (asked and unanswered, or
-- answered so).
CREATE TABLE program_tasks (
    task_id         INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
    run_id          INTEGER NOT NULL REFERENCES program_runs(id) ON DELETE CASCADE,
    step_key        TEXT    NOT NULL,
    iteration       INTEGER NOT NULL DEFAULT 1,
    param           TEXT,
    counts          INTEGER NOT NULL DEFAULT 1,
    time_limit_json TEXT,
    paused_at       INTEGER,
    skipped         INTEGER NOT NULL DEFAULT 0,
    question_json   TEXT,
    asked           INTEGER NOT NULL DEFAULT 0,
    answer          TEXT
);
CREATE INDEX program_tasks_run ON program_tasks(run_id);
