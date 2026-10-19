//! Platform-independent proxy for public GitHub Release attachments.
mod audit;
mod auth;
mod proxy;

pub use audit::{Audit, AuditEvent, AuditOutcome};
pub use auth::{ApiKeyRecord, ApiKeyStore};

pub use proxy::{HttpClient, ProxyError, ProxyRequest, ProxyResponse, error_response, proxy};
