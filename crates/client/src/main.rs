use clap::Parser;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::{
    Request, Response, StatusCode, Uri,
    body::{Bytes, Incoming},
    header::{CONNECTION, HOST, HeaderMap, HeaderName, HeaderValue},
    server::conn::http1,
    service::service_fn,
};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::{TokioExecutor, TokioIo, TokioTimer},
};
use std::{convert::Infallible, net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;

/// Forward local HTTP requests to a Holyproxy service.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Remote service URL, optionally including a path prefix
    #[arg(long)]
    remote: String,
    /// API key sent in X-Holyproxy-Key
    #[arg(long)]
    key: String,
    /// Local socket address to listen on
    #[arg(long, default_value = "127.0.0.1:10042")]
    listen: SocketAddr,
}

struct Proxy {
    remote: String,
    key: HeaderValue,
    client: Client<HttpsConnector<HttpConnector>, Incoming>,
}

type Body = BoxBody<Bytes, hyper::Error>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let remote: Uri = args.remote.parse().map_err(|_| "Invalid remote URL")?;
    if !matches!(remote.scheme_str(), Some("http" | "https"))
        || remote.authority().is_none_or(|a| a.as_str().contains('@'))
        || remote.query().is_some()
        || args.remote.contains('#')
    {
        return Err("Remote URL must use HTTP(S), without credentials, query or fragment".into());
    }
    let mut key = HeaderValue::from_str(&args.key).map_err(|_| "Invalid key header value")?;
    key.set_sensitive(true);
    let connector = HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .build();
    let proxy = Arc::new(Proxy {
        remote: remote.to_string().trim_end_matches('/').to_owned(),
        key,
        client: Client::builder(TokioExecutor::new())
            .pool_timer(TokioTimer::new())
            .retry_canceled_requests(false)
            .build(connector),
    });
    let listener = TcpListener::bind(args.listen).await?;
    eprintln!("Listening on http://{}", listener.local_addr()?);
    loop {
        let (stream, _) = tokio::select! {
            accepted = listener.accept() => accepted?,
            _ = tokio::signal::ctrl_c() => return Ok(()),
        };
        let proxy = Arc::clone(&proxy);
        tokio::spawn(async move {
            let service = service_fn(move |request| forward(Arc::clone(&proxy), request));
            if http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
                .is_err()
            {
                eprintln!("Local connection closed with an HTTP or stream error");
            }
        });
    }
}

async fn forward(
    proxy: Arc<Proxy>,
    mut request: Request<Incoming>,
) -> Result<Response<Body>, Infallible> {
    let path = request
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    if !path.starts_with('/') {
        return Ok(error(StatusCode::BAD_REQUEST, "Expected a URL path"));
    }
    // Concatenation preserves escaped paths and query bytes without URL normalization.
    let Ok(uri) = format!("{}{path}", proxy.remote).parse::<Uri>() else {
        return Ok(error(StatusCode::BAD_REQUEST, "Invalid request URI"));
    };
    *request.uri_mut() = uri;
    strip_hop_headers(request.headers_mut());
    request.headers_mut().remove(HOST);
    request
        .headers_mut()
        .insert("x-holyproxy-key", proxy.key.clone());
    match proxy.client.request(request).await {
        Ok(mut response) => {
            strip_hop_headers(response.headers_mut());
            Ok(response.map(BodyExt::boxed))
        }
        Err(_) => Ok(error(StatusCode::BAD_GATEWAY, "Remote request failed")),
    }
}

// Connection-specific headers describe one transport hop, not the forwarded message.
fn strip_hop_headers(headers: &mut HeaderMap) {
    let nominated: Vec<HeaderName> = headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| name.trim().parse().ok())
        .collect();
    for name in nominated {
        headers.remove(name);
    }
    for name in [
        "connection",
        "keep-alive",
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ] {
        headers.remove(name);
    }
}

fn error(status: StatusCode, message: &'static str) -> Response<Body> {
    let mut response = Response::new(
        Full::new(Bytes::from_static(message.as_bytes()))
            .map_err(|never| match never {})
            .boxed(),
    );
    *response.status_mut() = status;
    response
}
