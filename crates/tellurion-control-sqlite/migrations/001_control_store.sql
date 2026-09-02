BEGIN IMMEDIATE;

CREATE TABLE control_schema (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    version INTEGER NOT NULL
);
INSERT INTO control_schema (singleton, version) VALUES (1, 1);

CREATE TABLE control_revisions (
    revision INTEGER PRIMARY KEY CHECK (revision > 0),
    snapshot_json TEXT NOT NULL,
    recorded_at_unix_ms INTEGER NOT NULL CHECK (recorded_at_unix_ms >= 0)
);

CREATE TABLE control_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    current_revision INTEGER NOT NULL REFERENCES control_revisions(revision)
);

CREATE TABLE control_resources (
    resource_key TEXT PRIMARY KEY,
    resource_kind TEXT NOT NULL,
    resource_json TEXT NOT NULL
);

CREATE TABLE control_role_bindings (
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    role TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    binding_json TEXT NOT NULL,
    PRIMARY KEY (issuer, subject, role, scope_key)
);

CREATE TABLE control_path_policies (
    policy_id TEXT PRIMARY KEY,
    policy_json TEXT NOT NULL
);

CREATE TABLE control_tombstones (
    scope_key TEXT PRIMARY KEY,
    scope_json TEXT NOT NULL
);

CREATE TABLE control_entity_versions (
    resource_key TEXT PRIMARY KEY,
    entity_version TEXT NOT NULL
);

CREATE TABLE control_audit (
    revision INTEGER PRIMARY KEY REFERENCES control_revisions(revision),
    actor_json TEXT NOT NULL,
    request_json TEXT NOT NULL,
    idempotency_key TEXT,
    changed_resources_json TEXT NOT NULL,
    recorded_at_unix_ms INTEGER NOT NULL CHECK (recorded_at_unix_ms >= 0),
    applying_instance TEXT NOT NULL
);

CREATE TABLE control_outbox (
    revision INTEGER NOT NULL REFERENCES control_revisions(revision),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    changed_resources_json TEXT NOT NULL,
    PRIMARY KEY (revision, ordinal)
);

CREATE TABLE control_idempotency (
    idempotency_key TEXT PRIMARY KEY,
    changeset_json TEXT NOT NULL,
    commit_json TEXT NOT NULL
);

PRAGMA user_version = 1;
COMMIT;
