PRAGMA foreign_keys = ON;
PRAGMA user_version = 19;

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
  folder_id TEXT NOT NULL DEFAULT 'folder-default'
);

CREATE TABLE session_folder (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  sort_order INTEGER NOT NULL DEFAULT 0,
  collapsed INTEGER NOT NULL DEFAULT 0,
  locked INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
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
  PRIMARY KEY (id, session_id)
);

CREATE TABLE checkpoint (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  turn_idx INTEGER NOT NULL,
  label TEXT NOT NULL,
  snapshot_ref TEXT NOT NULL,
  ts INTEGER NOT NULL
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

INSERT INTO session_folder (id, name, sort_order, collapsed, locked, created_at)
VALUES ('folder-default', '未归类', 0, 0, 1, 1717171700000);

INSERT INTO session
  (id, cli_session_id, title, engine, model, cwd, status, created_at, updated_at, provider_id,
   folder_id)
VALUES
  ('session-v19', 'thread-v19', 'v19 合成会话', 'claude-code', 'claude-fixture',
   'D:\work\legacy', 'idle', 1717171700, 1717171701, 'provider-fixture', 'folder-default');

INSERT INTO message (session_id, role, text, ts, reverted, turn_id)
VALUES ('session-v19', 'user', 'fixture-v19', 1717171700000, 0, 'turn-v19-1');
