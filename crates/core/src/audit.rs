/// The final HTTP outcome, not confirmation that the client received the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditOutcome {
    Success,
    Failure,
}

/// One completed incoming request. Identity comes exclusively from the allowlist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditEvent {
    pub username: Option<String>,
    pub outcome: AuditOutcome,
}

/// Notify the adapter without delaying or failing the response.
/// Implementations own scheduling, persistence, and reporting write failures.
pub trait Audit {
    fn record(&self, event: AuditEvent);
}
