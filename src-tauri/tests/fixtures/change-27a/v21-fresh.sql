PRAGMA foreign_keys = ON;
PRAGMA user_version = 21;

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

CREATE TABLE session_folder (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  sort_order INTEGER NOT NULL DEFAULT 0,
  collapsed INTEGER NOT NULL DEFAULT 0,
  locked INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  cwd TEXT,
  cwd_key TEXT
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

CREATE TABLE approval (
  id TEXT NOT NULL,
  session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  action TEXT NOT NULL,
  detail TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending',
  ts INTEGER NOT NULL,
  decision TEXT,
  rule_id TEXT,
  error TEXT,
  resolved_at INTEGER,
  persistent_label TEXT,
  matcher_summary TEXT,
  turn_id TEXT,
  PRIMARY KEY (id, session_id)
);

CREATE TABLE turn_snapshot (
  history_session_id TEXT PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,
  turn_id TEXT NOT NULL,
  turn_epoch INTEGER NOT NULL,
  status TEXT NOT NULL,
  terminal_reason TEXT,
  recoverable INTEGER NOT NULL DEFAULT 1,
  event_seq INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL,
  turn_mode TEXT NOT NULL DEFAULT 'build',
  permission_profile TEXT NOT NULL DEFAULT 'standard',
  started_at INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE turn (
  history_session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  turn_id TEXT NOT NULL,
  turn_epoch INTEGER NOT NULL,
  turn_mode TEXT NOT NULL,
  permission_profile TEXT NOT NULL,
  status TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  ended_at INTEGER,
  terminal_reason TEXT,
  PRIMARY KEY (history_session_id, turn_id)
);

INSERT INTO session_folder
  (id, name, sort_order, collapsed, locked, created_at, cwd, cwd_key)
VALUES
  ('folder-default', '未归类', 0, 0, 1, 1717171700000, NULL, NULL),
  ('folder-project', 'demo', 10, 0, 0, 1717171700000, 'D:\work\demo', 'd:/work/demo');

INSERT INTO session
  (id, cli_session_id, title, engine, model, cwd, status, created_at, updated_at,
   provider_id, folder_id)
VALUES
  ('session-v21', 'thread-v21', '合成会话', 'codex', 'gpt-fixture', 'D:\work\demo',
   'idle', 1717171700, 1717171701, 'provider-fixture', 'folder-project');

INSERT INTO turn
  (history_session_id, turn_id, turn_epoch, turn_mode, permission_profile, status, started_at,
   ended_at, terminal_reason)
VALUES
  ('session-v21', 'turn-v21-1', 1, 'build', 'standard', 'succeeded', 1717171700000,
   1717171701000, 'end');

INSERT INTO message (session_id, role, text, ts, reverted, turn_id)
VALUES
  ('session-v21', 'user', 'fixture-user', 1717171700000, 0, 'turn-v21-1'),
  ('session-v21', 'assistant', 'fixture-assistant', 1717171701000, 0, 'turn-v21-1');
