-- 0002 — Reflexion 反思记忆表（Phase 5）
-- 任务结束后的反思记录，存 SQLite 供下一轮 chat 检索 + 注入 system prompt

CREATE TABLE IF NOT EXISTS reflexion_memory (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    task_run_id     TEXT,
    task_title      TEXT NOT NULL,
    task_status     TEXT NOT NULL,                -- ok | failed | in_progress
    what_went_wrong TEXT,                          -- 错误详情
    why             TEXT,                          -- 错误分类（timeout / verify_failed / ...）
    improvement     TEXT,                          -- 改进建议
    ts_ms           INTEGER NOT NULL,
    extra           TEXT                           -- JSON
);

CREATE INDEX IF NOT EXISTS idx_reflexion_ts ON reflexion_memory(ts_ms DESC);
CREATE INDEX IF NOT EXISTS idx_reflexion_task_run ON reflexion_memory(task_run_id);
