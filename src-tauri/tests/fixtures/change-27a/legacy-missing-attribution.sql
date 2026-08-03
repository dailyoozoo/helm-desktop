PRAGMA foreign_keys = ON;
PRAGMA user_version = 21;

-- 该样本刻意只定义 legacy 读取需要的最小表形状。空字符串表示旧行没有可靠的
-- Provider/Model 归属；NULL turn_id 表示无法可靠关联 Turn。迁移不得猜填这些值。
CREATE TABLE session (
  id TEXT PRIMARY KEY,
  cli_session_id TEXT UNIQUE,
  title TEXT NOT NULL,
  engine TEXT NOT NULL,
  model TEXT NOT NULL,
  cwd TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  summary TEXT,
  provider_id TEXT NOT NULL DEFAULT '',
  pinned INTEGER NOT NULL DEFAULT 0,
  runtime_capabilities_json TEXT,
  safe_permission_profile TEXT NOT NULL DEFAULT 'standard',
  folder_id TEXT NOT NULL DEFAULT 'folder-default',
  last_context_tokens INTEGER,
  last_context_window INTEGER
);

CREATE TABLE message (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  text TEXT NOT NULL,
  ts INTEGER NOT NULL,
  reverted INTEGER NOT NULL DEFAULT 0,
  turn_id TEXT
);

CREATE TABLE tool_call (
  id TEXT NOT NULL,
  session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  input_json TEXT NOT NULL,
  status TEXT NOT NULL,
  output TEXT,
  diff_json TEXT,
  ts INTEGER NOT NULL,
  ended_at INTEGER,
  turn_id TEXT,
  PRIMARY KEY (id, session_id)
);

CREATE TABLE usage (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  model TEXT NOT NULL,
  provider_id TEXT NOT NULL DEFAULT '',
  input_tokens INTEGER NOT NULL,
  cached_input_tokens INTEGER NOT NULL DEFAULT 0,
  cache_write_input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL,
  cost_usd REAL NOT NULL,
  reported_cost_usd REAL,
  cost_kind TEXT NOT NULL DEFAULT 'legacy',
  price_source TEXT NOT NULL DEFAULT 'legacy',
  service_tier TEXT NOT NULL DEFAULT 'standard',
  pricing_catalog_version TEXT,
  price_snapshot_json TEXT,
  ts INTEGER NOT NULL
);

CREATE TABLE checkpoint (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  turn_idx INTEGER NOT NULL,
  label TEXT NOT NULL,
  snapshot_ref TEXT NOT NULL,
  ts INTEGER NOT NULL,
  turn_id TEXT
);

INSERT INTO session
  (id, cli_session_id, title, engine, model, cwd, status, created_at, updated_at, provider_id,
   folder_id)
VALUES
  ('session-legacy', NULL, '归属缺失样本', 'codex', '', 'D:\work\legacy', 'idle',
   1717171700, 1717171701, '', 'folder-default');

INSERT INTO message (session_id, role, text, ts, reverted, turn_id)
VALUES
  ('session-legacy', 'user', 'legacy-user', 1717171700000, 0, NULL),
  ('session-legacy', 'assistant', 'legacy-assistant', 1717171701000, 0, NULL);

INSERT INTO tool_call
  (id, session_id, name, input_json, status, output, diff_json, ts, ended_at, turn_id)
VALUES
  ('tool-legacy', 'session-legacy', 'Read', '{"file_path":"fixture.txt"}', 'success',
   'fixture-output', NULL, 1717171700200, 1717171700300, NULL);

INSERT INTO usage
  (session_id, model, provider_id, input_tokens, output_tokens, cost_usd, cost_kind,
   price_source, ts)
VALUES
  ('session-legacy', '', '', 10, 2, 0, 'legacy', 'legacy', 1717171701000);

INSERT INTO checkpoint (id, session_id, turn_idx, label, snapshot_ref, ts, turn_id)
VALUES
  ('checkpoint-legacy', 'session-legacy', 1, 'legacy', 'fixture-snapshot', 1717171700400, NULL);
