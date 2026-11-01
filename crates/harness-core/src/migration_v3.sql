CREATE TABLE patch_attempts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    call_id INTEGER NOT NULL REFERENCES tool_calls(id),
    approval_id TEXT NOT NULL,
    binding TEXT NOT NULL,
    intent TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('intent','staged','applied','unchanged','unknown')),
    evidence TEXT
);
UPDATE schema_meta SET contract='z-m2-v3' WHERE singleton=1;
PRAGMA user_version=3;
