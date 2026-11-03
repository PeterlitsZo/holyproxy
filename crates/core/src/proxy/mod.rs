use crate::audit::{Audit, AuditEvent, AuditOutcome};
use crate::auth::{ApiKeyStore, credentials};
use std::future::Future;
use url::Url;

mod routing;

use routing::{redirect_url, release_url};

/// Read incoming request metadata without consuming its body.
pub trait ProxyRequest {
    type Error;

    fn method(&self) -> String;
    fn url(&self) -> Result<Url, Self::Error>;
    fn header(&self, name: &str) -> Result<Option<String>, Self::Error>;
}

/// Response metadata and text construction. Upstream bodies remain opaque.
pub trait ProxyResponse: Sized {
    type Error;

    fn status(&self) -> u16;
    fn header(&self, name: &str) -> Result<Option<String>, Self::Error>;
    fn text(status: u16, body: &str, headers: &[(&str, &str)]) -> Result<Self, Self::Error>;
}

/// Send individual HTTP requests while preserving streaming response bodies.
pub trait HttpClient {
    type Error;
    type Response: ProxyResponse<Error = Self::Error>;

    /// Do not automatically follow redirects or buffer the response body.
    fn fetch(
        &self,
        url: &Url,
        method: &str,
        headers: &[(String, String)],
    ) -> impl Future<Output = Result<Self::Response, Self::Error>>;
}

/// Proxy policy failures or an original runtime error for adapter-side logging.
#[derive(Debug, PartialEq)]
pub enum ProxyError<E> {
    Forbidden,
    AuthStore(E),
    UnsupportedMethod,
    InvalidTarget,
    InvalidRedirect,
    Runtime(E),
}

/// Construct the public error response without exposing runtime error details.
pub fn error_response<R: ProxyResponse>(error: ProxyError<R::Error>) -> Result<R, R::Error> {
    match error {
        ProxyError::Forbidden => R::text(403, "Forbidden", &[("Cache-Control", "no-store")]),
        ProxyError::AuthStore(_) => R::text(
            503,
            "Authorization service unavailable",
            &[("Cache-Control", "no-store")],
        ),
        ProxyError::UnsupportedMethod => R::text(
            405,
            "Only GET and HEAD are supported",
            &[("Allow", "GET, HEAD")],
        ),
        ProxyError::InvalidTarget => R::text(400, "Expected a GitHub Release attachment URL", &[]),
        ProxyError::InvalidRedirect => {
            R::text(502, "Invalid upstream redirect or too many redirects", &[])
        }
        ProxyError::Runtime(_) => R::text(502, "Upstream request failed", &[]),
    }
}

/// Proxy a public Release attachment, including validation and download redirects.
pub async fn proxy<C: HttpClient>(
    client: &C,
    keys: &impl ApiKeyStore<Error = C::Error>,
    audit: &impl Audit,
    request: impl ProxyRequest<Error = C::Error>,
) -> Result<C::Response, ProxyError<C::Error>> {
    let mut username = None;
    let result = proxy_inner(client, keys, request, &mut username).await;
    let outcome = match &result {
        Ok(response) if (200..400).contains(&response.status()) => AuditOutcome::Success,
        _ => AuditOutcome::Failure,
    };
    audit.record(AuditEvent { username, outcome });
    result
}

async fn proxy_inner<C: HttpClient>(
    client: &C,
    key_store: &impl ApiKeyStore<Error = C::Error>,
    request: impl ProxyRequest<Error = C::Error>,
    username: &mut Option<String>,
) -> Result<C::Response, ProxyError<C::Error>> {
    let mut incoming = request.url().map_err(ProxyError::Runtime)?;

    // Extract the credential.
    let header = request
        .header("X-Holyproxy-Key")
        .map_err(ProxyError::Runtime)?;
    let authorization = request
        .header("Authorization")
        .map_err(ProxyError::Runtime)?;
    let (digest, query) = credentials(
        header.as_deref(),
        incoming.query(),
        authorization.as_deref(),
    )
    .ok_or(ProxyError::Forbidden)?;

    // Check if the user have the right to access this proxy.
    let identity = key_store
        .contains(&digest)
        .await
        .map_err(ProxyError::AuthStore)?
        .ok_or(ProxyError::Forbidden)?;
    *username = Some(identity.username);

    // Change the query.
    incoming.set_query(query.as_deref());

    // Only support GET and HEAD now.
    let method = request.method();
    if !matches!(method.as_str(), "GET" | "HEAD") {
        return Err(ProxyError::UnsupportedMethod);
    }

    // Build the URL.
    let Some(mut target) = release_url(incoming.path(), incoming.query()) else {
        return Err(ProxyError::InvalidTarget);
    };

    // Build the headers.
    let mut headers = vec![("User-Agent".into(), "holyproxy".into())];
    for name in DOWNLOAD_HEADERS {
        if let Some(value) = request.header(name).map_err(ProxyError::Runtime)? {
            headers.push(((*name).into(), value));
        }
    }

    // Do the proxy.
    for hop in 0..=10 {
        let response = client
            .fetch(&target, &method, &headers)
            .await
            .map_err(ProxyError::Runtime)?;

        if !is_redirect(response.status()) {
            return Ok(response);
        }

        let location = response.header("Location").map_err(ProxyError::Runtime)?;
        let next = location
            .as_deref()
            .and_then(|value| redirect_url(&target, value));

        match next {
            Some(next) if hop < 10 => target = next,
            _ => return Err(ProxyError::InvalidRedirect),
        }
    }

    unreachable!()
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Headers needed for public downloads and conditional/range requests.
const DOWNLOAD_HEADERS: &[&str] = &[
    "accept",
    "range",
    "if-range",
    "if-match",
    "if-none-match",
    "if-modified-since",
    "if-unmodified-since",
];

#[cfg(test)]
mod tests;
