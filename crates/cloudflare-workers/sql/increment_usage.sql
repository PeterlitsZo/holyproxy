INSERT INTO usage_buckets (
    bucket_start, principal_type, username, requests, successes, failures
) VALUES (?1, ?2, ?3, 1, ?4, ?5)
ON CONFLICT (bucket_start, principal_type, username) DO UPDATE SET
    requests = usage_buckets.requests + 1,
    successes = usage_buckets.successes + excluded.successes,
    failures = usage_buckets.failures + excluded.failures;
