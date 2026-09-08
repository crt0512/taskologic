-- Timestamps are unix seconds, UTC. Users are keyed by uid, not name.

CREATE TABLE users (
    uid           INTEGER PRIMARY KEY,
    username      TEXT    NOT NULL,
    is_admin      INTEGER NOT NULL DEFAULT 0,
    timezone      TEXT    NOT NULL,
    prefs_json    TEXT    NOT NULL DEFAULT '{}',
    pin_hash      TEXT,
    created_at    INTEGER NOT NULL,
    last_login_at INTEGER
);

CREATE TABLE boards (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    name               TEXT    NOT NULL,
    owner_uid          INTEGER NOT NULL,
    is_locked          INTEGER NOT NULL DEFAULT 0,
    is_private         INTEGER NOT NULL DEFAULT 0,
    archive_after_secs INTEGER NOT NULL,
    -- Nullable only between inserting the board and its columns.
    started_col        INTEGER,
    paused_col         INTEGER,
    finished_col       INTEGER,
    created_at         INTEGER NOT NULL
);

CREATE TABLE board_members (
    board_id INTEGER NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
    uid      INTEGER NOT NULL,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (board_id, uid)
);

CREATE TABLE columns (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    board_id INTEGER NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
    name     TEXT    NOT NULL,
    position INTEGER NOT NULL
);
CREATE INDEX columns_board ON columns(board_id, position);

CREATE TABLE tasks (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    short_id          TEXT    NOT NULL UNIQUE,
    board_id          INTEGER NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
    column_id         INTEGER NOT NULL REFERENCES columns(id),
    position          INTEGER NOT NULL,
    title             TEXT    NOT NULL,
    description       TEXT    NOT NULL DEFAULT '',
    due_at            INTEGER,
    created_by        INTEGER NOT NULL,
    created_at        INTEGER NOT NULL,
    finished_at       INTEGER,
    version           INTEGER NOT NULL DEFAULT 1,
    archived_at       INTEGER,
    archived_from_col INTEGER
);
CREATE INDEX tasks_board ON tasks(board_id, column_id, position);
CREATE INDEX tasks_finished ON tasks(finished_at) WHERE archived_at IS NULL;

CREATE TABLE assignees (
    task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    uid     INTEGER NOT NULL,
    PRIMARY KEY (task_id, uid)
);
CREATE INDEX assignees_uid ON assignees(uid);

-- Deleting either side drops the link, which is the spec for deleted deps.
CREATE TABLE deps (
    task_id            INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on_task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, depends_on_task_id)
);

CREATE TABLE repeats (
    task_id      INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
    rule_json    TEXT    NOT NULL,
    next_fire_at INTEGER,
    active       INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX repeats_due ON repeats(next_fire_at) WHERE active = 1;

CREATE TABLE templates (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    board_id     INTEGER REFERENCES boards(id) ON DELETE CASCADE,
    owner_uid    INTEGER NOT NULL,
    name         TEXT    NOT NULL,
    payload_json TEXT    NOT NULL
);

-- No foreign keys on purpose: the log outlives the rows it talks about.
CREATE TABLE events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     INTEGER,
    board_id    INTEGER NOT NULL,
    actor_uid   INTEGER,
    kind        TEXT    NOT NULL,
    from_col    INTEGER,
    to_col      INTEGER,
    detail_json TEXT    NOT NULL,
    at          INTEGER NOT NULL
);
CREATE INDEX events_task ON events(task_id, at);
CREATE INDEX events_board ON events(board_id, at);

CREATE TABLE print_queue (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    target_uid      INTEGER NOT NULL,
    job_json        TEXT    NOT NULL,
    created_at      INTEGER NOT NULL,
    expires_at      INTEGER NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    last_error      TEXT,
    in_flight_since INTEGER
);
CREATE INDEX print_queue_target ON print_queue(target_uid, created_at);

CREATE TABLE reminders_sent (
    task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    uid     INTEGER NOT NULL,
    due_at  INTEGER NOT NULL,
    sent_at INTEGER NOT NULL,
    PRIMARY KEY (task_id, uid, due_at)
);
