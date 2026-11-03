use crate::{ApiKeyStore, Audit, HttpClient, ProxyError, ProxyRequest, ProxyResponse, proxy};

/// Route the public homepage and authenticated proxy requests independently of the runtime.
pub async fn handle_request<C: HttpClient>(
    client: &C,
    keys: &impl ApiKeyStore<Error = C::Error>,
    audit: &impl Audit,
    request: impl ProxyRequest<Error = C::Error, Body = C::Body>,
) -> Result<C::Response, ProxyError<C::Error>> {
    if request.url().is_ok_and(|url| url.path() == "/") {
        let page = holyproxy_web::index(&request.method());
        return C::Response::text(page.status, page.body, page.headers)
            .map_err(ProxyError::Runtime);
    }

    // Let the proxy preserve its error handling and audit for URL failures too.
    proxy(client, keys, audit, request).await
}
