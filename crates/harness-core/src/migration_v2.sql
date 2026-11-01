ALTER TABLE turns ADD COLUMN output_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE turns ADD COLUMN tool_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE model_attempts ADD COLUMN metadata TEXT;
ALTER TABLE model_attempts ADD COLUMN started_unix_ms INTEGER;
CREATE TABLE protocol_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    turn_id INTEGER NOT NULL REFERENCES turns(id),
    epoch_id INTEGER NOT NULL REFERENCES epochs(id),
    payload TEXT NOT NULL
);
CREATE TABLE tool_calls (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    turn_id INTEGER NOT NULL REFERENCES turns(id),
    batch_id INTEGER NOT NULL REFERENCES protocol_messages(id),
    ordinal INTEGER NOT NULL,
    provider_id TEXT NOT NULL,
    name TEXT NOT NULL,
    arguments TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('proposed','intent','resolved')),
    result TEXT,
    UNIQUE(batch_id, ordinal),
    UNIQUE(turn_id, provider_id)
);
CREATE TABLE checkpoints (
    turn_id INTEGER PRIMARY KEY REFERENCES turns(id),
    closed_message_id INTEGER NOT NULL REFERENCES protocol_messages(id),
    active_ms INTEGER NOT NULL,
    output_bytes INTEGER NOT NULL,
    tool_count INTEGER NOT NULL
);
CREATE TABLE instructions (
    epoch_id INTEGER NOT NULL REFERENCES epochs(id),
    origin TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    content TEXT NOT NULL,
    PRIMARY KEY(epoch_id,origin,sha256)
);
UPDATE schema_meta SET contract='z-m1a-v2' WHERE singleton=1;
PRAGMA user_version=2;
