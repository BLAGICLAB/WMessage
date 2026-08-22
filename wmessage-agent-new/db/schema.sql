-- WMessage Agent Python kernel — DB schema
-- 自 wmessage-agent-spec.md §Z 数据模型

PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

-- ─────────────── session ───────────────
CREATE TABLE IF NOT EXISTS session (
    id              TEXT PRIMARY KEY,            -- UUID
    user_id         TEXT,
    channel         TEXT NOT NULL,               -- webchat | feishu | api | cli | tauri
    title           TEXT,
    created_at      INTEGER NOT NULL,            -- ms epoch
    last_active_at  INTEGER NOT NULL,
    metadata        TEXT                         -- JSON
);

CREATE INDEX IF NOT EXISTS idx_session_user_active
    ON session(user_id, last_active_at DESC);

-- ─────────────── session_message ───────────────
CREATE TABLE IF NOT EXISTS session_message (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT NOT NULL,
    role        TEXT NOT NULL,                   -- user | assistant | system | tool
    content     TEXT NOT NULL,
    ts_ms       INTEGER NOT NULL,
    meta        TEXT,                            -- JSON
    FOREIGN KEY (session_id) REFERENCES session(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_msg_session_ts
    ON session_message(session_id, ts_ms);

-- ─────────────── task_graph ───────────────
CREATE TABLE IF NOT EXISTS task_graph (
    id           TEXT PRIMARY KEY,               -- g-uuid
    session_id   TEXT NOT NULL,
    skill_run_id TEXT,
    root_query   TEXT NOT NULL,
    mode         TEXT NOT NULL,                  -- auto | interactive
    status       TEXT NOT NULL,                  -- planning | executing | verifying | replanning | done | failed
    plan_version INTEGER NOT NULL DEFAULT 1,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    FOREIGN KEY (session_id) REFERENCES session(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_graph_session
    ON task_graph(session_id, created_at DESC);

-- ─────────────── task_node ───────────────
CREATE TABLE IF NOT EXISTS task_node (
    id            TEXT PRIMARY KEY,              -- t-uuid
    graph_id      TEXT NOT NULL,
    idx           INTEGER NOT NULL,
    title         TEXT NOT NULL,
    deps          TEXT NOT NULL,                 -- JSON array of task_node.id
    verify_hint   TEXT,                          -- JSON
    status        TEXT NOT NULL,                 -- pending | in_progress | verify_passed | verify_failed | done | failed | skipped
    attempts      INTEGER NOT NULL DEFAULT 0,
    last_error    TEXT,
    result        TEXT,
    verify_log    TEXT,
    started_at    INTEGER,
    finished_at   INTEGER,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    FOREIGN KEY (graph_id) REFERENCES task_graph(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_node_graph_idx
    ON task_node(graph_id, idx);

CREATE INDEX IF NOT EXISTS idx_node_status
    ON task_node(status);

-- ─────────────── skill_run ───────────────
CREATE TABLE IF NOT EXISTS skill_run (
    id               TEXT PRIMARY KEY,           -- sr-uuid
    session_id       TEXT NOT NULL,
    graph_id         TEXT,
    skill_name       TEXT NOT NULL,
    mode             TEXT NOT NULL,
    risk             TEXT NOT NULL,
    status           TEXT NOT NULL,              -- running | done | failed | terminated
    started_at       INTEGER NOT NULL,
    finished_at      INTEGER,
    total_steps      INTEGER NOT NULL,
    completed_steps  INTEGER NOT NULL DEFAULT 0,
    replan_count     INTEGER NOT NULL DEFAULT 0,
    end_reason       TEXT,                       -- ok | timeout | error | user_cancel | verify_failed | replan_exhausted
    ctx              TEXT NOT NULL DEFAULT '[]', -- JSON CompletedStep[]
    error            TEXT,
    FOREIGN KEY (session_id) REFERENCES session(id) ON DELETE CASCADE,
    FOREIGN KEY (graph_id) REFERENCES task_graph(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_skillrun_session
    ON skill_run(session_id, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_skillrun_status
    ON skill_run(status);

-- ─────────────── audit_event ───────────────
CREATE TABLE IF NOT EXISTS audit_event (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    ts_ms         INTEGER NOT NULL,
    level         TEXT NOT NULL,                 -- Info | Warn | Error
    event         TEXT NOT NULL,                 -- user.message | pre_step.route_skill | ...
    session_id    TEXT,
    skill_run_id  TEXT,
    graph_id      TEXT,
    task_node_id  TEXT,
    payload       TEXT NOT NULL,                 -- JSON
    message       TEXT,                          -- 人类可读摘要（≤ 500 字）
    FOREIGN KEY (skill_run_id) REFERENCES skill_run(id) ON DELETE SET NULL,
    FOREIGN KEY (graph_id) REFERENCES task_graph(id) ON DELETE SET NULL,
    FOREIGN KEY (task_node_id) REFERENCES task_node(id) ON DELETE SET NULL,
    FOREIGN KEY (session_id) REFERENCES session(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_audit_session_ts
    ON audit_event(session_id, ts_ms);

CREATE INDEX IF NOT EXISTS idx_audit_event_ts
    ON audit_event(event, ts_ms);

CREATE INDEX IF NOT EXISTS idx_audit_level_ts
    ON audit_event(level, ts_ms);
