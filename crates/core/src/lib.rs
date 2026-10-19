//! Platform-independent proxy for public GitHub Release attachments.
mod proxy;

pub use proxy::{HttpClient, ProxyError, ProxyRequest, ProxyResponse, error_response, proxy};
