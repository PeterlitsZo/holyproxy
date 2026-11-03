use crate::audit::{Audit, AuditEvent, AuditOutcome};
use crate::auth::{ApiKeyStore, credentials};
use std::future::Future;
use url::Url;

mod routing;

use routing::{TargetKind, git_redirect_url, redirect_url, target_url};

/// Read metadata independently of the runtime-owned request body.
pub trait ProxyRequest {
    type Error;
    type Body;

    /// Called at most once, after authentication and route validation. Do not buffer.
    fn take_body(&mut self) -> Result<Option<Self::Body>, Self::Error>;

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
    type Body;
    type Response: ProxyResponse<Error = Self::Error>;

    /// Do not automatically follow redirects or buffer the response body.
    fn fetch(
        &self,
        url: &Url,
        method: &str,
        headers: &[(String, String)],
        body: Option<Self::Body>,
    ) -> impl Future<Output = Result<Self::Response, Self::Error>>;
}

/// Proxy policy failures or an original runtime error for adapter-side logging.
#[derive(Debug, PartialEq)]
pub enum ProxyError<E> {
    AuthenticationRequired,
    Forbidden,
    AuthStore(E),
    UnsupportedMethod(&'static str),
    UnsupportedMediaType,
    InvalidTarget,
    InvalidRedirect,
    Runtime(E),
}

/// Construct the public error response without exposing runtime error details.
pub fn error_response<R: ProxyResponse>(error: ProxyError<R::Error>) -> Result<R, R::Error> {
    match error {
        ProxyError::AuthenticationRequired => R::text(
            401,
            "Authentication required",
            &[
                ("WWW-Authenticate", "Basic realm=\"holyproxy\""),
                ("Cache-Control", "no-store"),
            ],
        ),
        ProxyError::Forbidden => R::text(403, "Forbidden", &[("Cache-Control", "no-store")]),
        ProxyError::AuthStore(_) => R::text(
            503,
            "Authorization service unavailable",
            &[("Cache-Control", "no-store")],
        ),
        ProxyError::UnsupportedMethod(allow) => R::text(
            405,
            "Method not allowed for this endpoint",
            &[("Allow", allow)],
        ),
        ProxyError::UnsupportedMediaType => {
            R::text(415, "Expected application/x-git-upload-pack-request", &[])
        }
        ProxyError::InvalidTarget => R::text(
            400,
            "Expected a GitHub Release attachment or Git Smart HTTP URL",
            &[],
        ),
        ProxyError::InvalidRedirect => {
            R::text(502, "Invalid upstream redirect or too many redirects", &[])
        }
        ProxyError::Runtime(_) => R::text(502, "Upstream request failed", &[]),
    }
}

/// Proxy a public Release attachment or Git Smart HTTP request.
pub async fn proxy<C: HttpClient>(
    client: &C,
    keys: &impl ApiKeyStore<Error = C::Error>,
    audit: &impl Audit,
    request: impl ProxyRequest<Error = C::Error, Body = C::Body>,
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
    mut request: impl ProxyRequest<Error = C::Error, Body = C::Body>,
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
    // Git first probes without credentials. Challenge only recognized Git routes
    // with no supplied credentials; malformed or conflicting keys still fail closed.
    if header.is_none()
        && authorization.is_none()
        && !incoming
            .query_pairs()
            .any(|(name, _)| name == "x_holyproxy_key")
        && target_url(incoming.path(), incoming.query())
            .is_some_and(|(_, kind)| kind != TargetKind::Release)
    {
        return Err(ProxyError::AuthenticationRequired);
    }
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

    // Check the original URL by target kind and get the target URL.
    let method = request.method();
    let (mut target, kind) =
        target_url(incoming.path(), incoming.query()).ok_or(ProxyError::InvalidTarget)?;
    if !kind
        .allowed_methods()
        .split(", ")
        .any(|allowed| allowed == method)
    {
        return Err(ProxyError::UnsupportedMethod(kind.allowed_methods()));
    }
    if kind == TargetKind::GitUploadPack {
        let content_type = request
            .header("Content-Type")
            .map_err(ProxyError::Runtime)?;
        if !content_type.as_deref().is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .eq_ignore_ascii_case("application/x-git-upload-pack-request")
        }) {
            return Err(ProxyError::UnsupportedMediaType);
        }
    }

    // Build the headers.
    let mut headers = vec![("User-Agent".into(), "holyproxy".into())];
    for name in if kind == TargetKind::Release {
        DOWNLOAD_HEADERS
    } else {
        GIT_HEADERS
    } {
        if let Some(value) = request.header(name).map_err(ProxyError::Runtime)? {
            headers.push(((*name).into(), value));
        }
    }

    let mut body = if kind == TargetKind::GitUploadPack {
        request.take_body().map_err(ProxyError::Runtime)?
    } else {
        None
    };

    // Do the proxy.
    for hop in 0..=10 {
        let response = client
            .fetch(&target, &method, &headers, body.take())
            .await
            .map_err(ProxyError::Runtime)?;

        if !is_redirect(response.status()) {
            return Ok(response);
        }

        if kind == TargetKind::GitUploadPack {
            return Err(ProxyError::InvalidRedirect);
        }

        let location = response.header("Location").map_err(ProxyError::Runtime)?;
        let next = location.as_deref().and_then(|value| {
            if kind == TargetKind::Release {
                redirect_url(&target, value)
            } else {
                git_redirect_url(&target, value)
            }
        });

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

const GIT_HEADERS: &[&str] = &["accept", "content-type", "git-protocol", "content-encoding"];

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
