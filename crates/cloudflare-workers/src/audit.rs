use holyproxy_core::{Audit, AuditEvent, AuditOutcome};
use worker::*;

pub(crate) struct WorkersAudit {
    env: Env,
    ctx: Context,
    bucket_start: u64,
}

impl WorkersAudit {
    pub(crate) fn new(env: Env, ctx: Context, timestamp: u64) -> Self {
        Self {
            env,
            ctx,
            bucket_start: bucket_start(timestamp),
        }
    }
}

impl Audit for WorkersAudit {
    fn record(&self, event: AuditEvent) {
        let env = self.env.clone();
        let bucket_start = self.bucket_start;
        self.ctx.wait_until(async move {
            if increment(&env, bucket_start, event).await.is_err() {
                console_error!("Usage audit write failed");
            }
        });
    }
}

async fn increment(env: &Env, bucket_start: u64, event: AuditEvent) -> Result<()> {
    let db = env.d1("HOLYPROXY")?;
    let principal_type = if event.username.is_some() {
        "user"
    } else {
        "anonymous"
    };
    let username = event.username.unwrap_or_default();
    let successes = i32::from(event.outcome == AuditOutcome::Success);
    let result = db
        .prepare(include_str!("../sql/increment_usage.sql"))
        .bind(&[
            (bucket_start as f64).into(),
            principal_type.into(),
            username.into(),
            successes.into(),
            (1 - successes).into(),
        ])?
        .run()
        .await?;
    if !result.success() {
        return Err(Error::RustError("Usage audit write failed".into()));
    }
    Ok(())
}

fn bucket_start(timestamp: u64) -> u64 {
    timestamp / 900 * 900
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floors_quarter_hours_and_midnight() {
        for boundary in [0, 900, 1800, 2700, 3600, 86400, 172800] {
            assert_eq!(bucket_start(boundary), boundary);
            assert_eq!(bucket_start(boundary + 899), boundary);
            if boundary > 0 {
                assert_eq!(bucket_start(boundary - 1), boundary - 900);
            }
        }
    }

    #[test]
    fn local_days_align_for_fractional_hour_timezones() {
        for offset in [5 * 3600 + 30 * 60, 5 * 3600 + 45 * 60] {
            let start = 86400 - offset;
            let end = start + 86400;
            assert_eq!(bucket_start(start), start);
            assert_eq!(bucket_start(end), end);
            assert_eq!((start..end).step_by(900).count(), 96);
            assert!(bucket_start(start - 1) < start);
            assert!(bucket_start(end - 1) < end);
        }
    }
}
