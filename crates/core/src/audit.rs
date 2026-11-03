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
    pub method: Option<String>,
    pub path: Option<String>,
}

/// Notify the adapter without delaying or failing the response.
/// Implementations own scheduling, persistence, and reporting write failures.
pub trait Audit {
    fn record(&self, event: AuditEvent);
}

/// Bound untrusted metadata without retaining query, credentials, or request bodies.
pub(crate) fn sample_method(method: &str) -> Option<String> {
    (!method.is_empty()
        && method.len() <= 32
        && method
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)))
    .then(|| method.to_owned())
}

pub(crate) fn sample_path(path: &str) -> String {
    let mut end = path.len().min(1024);
    while !path.is_char_boundary(end) {
        end -= 1;
    }
    if end < path.len() {
        // Do not cut a percent-encoded byte in half.
        if let Some(percent) = path[..end].rfind('%') {
            if end - percent < 3 {
                end = percent;
            }
        }
    }
    path[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_methods_and_paths_without_decoding() {
        for method in ["GET", "HEAD", "POST", "CUSTOM-VERB"] {
            assert_eq!(sample_method(method).as_deref(), Some(method));
        }
        for method in ["", "GET POST", "GET\n", "非ASCII", &"A".repeat(33)] {
            assert_eq!(sample_method(method), None);
        }
        assert_eq!(sample_method(&"A".repeat(32)).unwrap().len(), 32);
        assert_eq!(sample_path("/%2f/%E4%B8%AD"), "/%2f/%E4%B8%AD");
        assert_eq!(sample_path(&"a".repeat(1025)).len(), 1024);
        assert_eq!(
            sample_path(&format!("{}中", "a".repeat(1023))),
            "a".repeat(1023)
        );
        for prefix in [1022, 1023] {
            assert_eq!(
                sample_path(&format!("{}%2F", "a".repeat(prefix))),
                "a".repeat(prefix)
            );
        }
        assert_eq!(
            sample_path(&format!("{}%2Fmore", "a".repeat(1021))).len(),
            1024
        );
    }
}
