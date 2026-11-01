CREATE TABLE schema_meta (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    contract TEXT NOT NULL
);
INSERT INTO schema_meta VALUES(1, 'z-m0-v1');

CREATE TABLE sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace TEXT NOT NULL,
    provider TEXT NOT NULL,
    account_ref TEXT NOT NULL,
    model TEXT NOT NULL,
    options TEXT NOT NULL,
    next_sequence INTEGER NOT NULL DEFAULT 1 CHECK(next_sequence > 0),
    UNIQUE(id, workspace)
);
CREATE TRIGGER immutable_session_binding
BEFORE UPDATE OF workspace, provider, account_ref, model, options ON sessions
BEGIN SELECT RAISE(ABORT, 'session binding is immutable'); END;

CREATE TABLE epochs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER NOT NULL REFERENCES sessions(id),
    state TEXT NOT NULL CHECK(state IN ('candidate', 'active', 'retired', 'rejected')),
    prefix TEXT NOT NULL,
    tool_schema TEXT NOT NULL,
    UNIQUE(id, session_id)
);
CREATE UNIQUE INDEX one_active_epoch ON epochs(session_id) WHERE state = 'active';
CREATE TRIGGER immutable_epoch_input
BEFORE UPDATE OF session_id, prefix, tool_schema ON epochs
BEGIN SELECT RAISE(ABORT, 'epoch input is immutable'); END;

CREATE TABLE turns (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id INTEGER NOT NULL,
    workspace TEXT NOT NULL,
    epoch_id INTEGER NOT NULL,
    state TEXT NOT NULL CHECK(state IN (
        'running', 'awaiting_approval', 'suspended_limit', 'suspended_recovery',
        'interrupted', 'completed', 'failed', 'refused', 'cancelled')),
    limits TEXT NOT NULL,
    active_ms INTEGER NOT NULL DEFAULT 0 CHECK(active_ms >= 0),
    outcome TEXT,
    FOREIGN KEY(session_id, workspace) REFERENCES sessions(id, workspace),
    FOREIGN KEY(epoch_id, session_id) REFERENCES epochs(id, session_id),
    UNIQUE(id, session_id),
    UNIQUE(id, epoch_id)
);
CREATE UNIQUE INDEX one_unresolved_workspace ON turns(workspace)
WHERE state IN ('running', 'awaiting_approval', 'suspended_limit', 'suspended_recovery', 'interrupted');
CREATE UNIQUE INDEX one_unresolved_session ON turns(session_id)
WHERE state IN ('running', 'awaiting_approval', 'suspended_limit', 'suspended_recovery', 'interrupted');

CREATE TABLE model_attempts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    turn_id INTEGER NOT NULL REFERENCES turns(id),
    state TEXT NOT NULL CHECK(state IN ('running', 'completed', 'incomplete', 'refused', 'failed', 'cancelled')),
    raw_usage TEXT,
    stop_reason TEXT
);
CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    turn_id INTEGER NOT NULL,
    epoch_id INTEGER NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('user', 'assistant')),
    content TEXT NOT NULL,
    FOREIGN KEY(turn_id, epoch_id) REFERENCES turns(id, epoch_id)
);
CREATE TABLE events (
    session_id INTEGER NOT NULL,
    sequence INTEGER NOT NULL CHECK(sequence > 0),
    turn_id INTEGER NOT NULL,
    payload TEXT NOT NULL,
    PRIMARY KEY(session_id, sequence),
    FOREIGN KEY(turn_id, session_id) REFERENCES turns(id, session_id)
);
