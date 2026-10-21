//! Platform-independent routing, homepage responses, and release downloads.
mod audit;
mod auth;
mod proxy;
mod router;

pub use audit::{Audit, AuditEvent, AuditOutcome};
pub use auth::{ApiKeyRecord, ApiKeyStore};

pub use proxy::{HttpClient, ProxyError, ProxyRequest, ProxyResponse, error_response, proxy};

pub use router::handle_request;
