INSERT INTO usage_buckets (
    bucket_start, principal_type, username, requests, successes, failures,
    last_method, last_path, last_seen_at
) VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7, ?8)
ON CONFLICT (bucket_start, principal_type, username) DO UPDATE SET
    requests = usage_buckets.requests + 1,
    successes = usage_buckets.successes + excluded.successes,
    failures = usage_buckets.failures + excluded.failures,
    last_method = CASE WHEN usage_buckets.last_seen_at IS NULL
        OR excluded.last_seen_at > usage_buckets.last_seen_at
        THEN excluded.last_method ELSE usage_buckets.last_method END,
    last_path = CASE WHEN usage_buckets.last_seen_at IS NULL
        OR excluded.last_seen_at > usage_buckets.last_seen_at
        THEN excluded.last_path ELSE usage_buckets.last_path END,
    last_seen_at = CASE WHEN usage_buckets.last_seen_at IS NULL
        OR excluded.last_seen_at > usage_buckets.last_seen_at
        THEN excluded.last_seen_at ELSE usage_buckets.last_seen_at END;
