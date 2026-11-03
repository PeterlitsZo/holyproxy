use holyproxy_core::{Audit, AuditEvent, AuditOutcome};
use worker::wasm_bindgen::JsValue;
use worker::*;

pub(crate) struct WorkersAudit {
    env: Env,
    ctx: Context,
    bucket_start: u64,
    received_at_ms: u64,
}

impl WorkersAudit {
    pub(crate) fn new(env: Env, ctx: Context, received_at_ms: u64) -> Self {
        Self {
            env,
            ctx,
            bucket_start: bucket_start(received_at_ms),
            received_at_ms,
        }
    }
}

impl Audit for WorkersAudit {
    fn record(&self, event: AuditEvent) {
        let env = self.env.clone();
        let bucket_start = self.bucket_start;
        let received_at_ms = self.received_at_ms;
        self.ctx.wait_until(async move {
            if increment(&env, bucket_start, received_at_ms, event)
                .await
                .is_err()
            {
                console_error!("Usage audit write failed");
            }
        });
    }
}

async fn increment(
    env: &Env,
    bucket_start: u64,
    received_at_ms: u64,
    event: AuditEvent,
) -> Result<()> {
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
            event.method.map(JsValue::from).unwrap_or(JsValue::NULL),
            event.path.map(JsValue::from).unwrap_or(JsValue::NULL),
            (received_at_ms as f64).into(),
        ])?
        .run()
        .await?;
    if !result.success() {
        return Err(Error::RustError("Usage audit write failed".into()));
    }
    Ok(())
}

fn bucket_start(received_at_ms: u64) -> u64 {
    received_at_ms / 900_000 * 900
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floors_quarter_hours_and_midnight() {
        for boundary in [0, 900, 1800, 2700, 3600, 86400, 172800] {
            assert_eq!(bucket_start(boundary * 1000), boundary);
            assert_eq!(bucket_start(boundary * 1000 + 899_999), boundary);
            if boundary > 0 {
                assert_eq!(bucket_start(boundary * 1000 - 1), boundary - 900);
            }
        }
    }

    #[test]
    fn local_days_align_for_fractional_hour_timezones() {
        for offset in [5 * 3600 + 30 * 60, 5 * 3600 + 45 * 60] {
            let start = 86400 - offset;
            let end = start + 86400;
            assert_eq!(bucket_start(start * 1000), start);
            assert_eq!(bucket_start(end * 1000), end);
            assert_eq!((start..end).step_by(900).count(), 96);
            assert!(bucket_start(start * 1000 - 1) < start);
            assert!(bucket_start(end * 1000 - 1) < end);
        }
    }
}
