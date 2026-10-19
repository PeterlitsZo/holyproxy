CREATE TABLE api_keys (
    key_hash TEXT PRIMARY KEY NOT NULL,
    username TEXT NOT NULL
);

CREATE TABLE usage_buckets (
    bucket_start INTEGER NOT NULL,
    principal_type TEXT NOT NULL,
    username TEXT NOT NULL,
    requests INTEGER NOT NULL DEFAULT 0,
    successes INTEGER NOT NULL DEFAULT 0,
    failures INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (bucket_start, principal_type, username)
);
