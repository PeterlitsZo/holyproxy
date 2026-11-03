ALTER TABLE usage_buckets ADD COLUMN last_method TEXT;
ALTER TABLE usage_buckets ADD COLUMN last_path TEXT;
ALTER TABLE usage_buckets ADD COLUMN last_seen_at INTEGER;
