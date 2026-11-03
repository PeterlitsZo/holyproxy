use super::*;
use crate::ApiKeyRecord;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures::executor::block_on;
use std::cell::RefCell;
use std::collections::VecDeque;

struct Request {
    method: String,
    url: Url,
    headers: Vec<(String, String)>,
    upload: Option<Vec<u8>>,
}

// Deliberately neither Clone nor readable through ProxyResponse.
#[derive(Debug)]
struct Body(Box<str>);

#[derive(Debug)]
struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Body,
}

#[derive(Default)]
struct MockClient {
    responses: RefCell<VecDeque<Result<Response, &'static str>>>,
    requests: RefCell<Vec<Request>>,
}

impl ProxyRequest for Request {
    type Error = &'static str;
    type Body = Vec<u8>;
    fn take_body(&mut self) -> Result<Option<Self::Body>, Self::Error> {
        Ok(self.upload.take())
    }

    fn method(&self) -> String {
        self.method.clone()
    }

    fn url(&self) -> Result<Url, Self::Error> {
        Ok(self.url.clone())
    }

    fn header(&self, name: &str) -> Result<Option<String>, Self::Error> {
        Ok(header(&self.headers, name))
    }
}

impl ProxyResponse for Response {
    type Error = &'static str;

    fn status(&self) -> u16 {
        self.status
    }

    fn header(&self, name: &str) -> Result<Option<String>, Self::Error> {
        Ok(header(&self.headers, name))
    }

    fn text(status: u16, body: &str, headers: &[(&str, &str)]) -> Result<Self, Self::Error> {
        Ok(Self {
            status,
            body: Body(body.into()),
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        })
    }
}

impl HttpClient for MockClient {
    type Error = &'static str;
    type Response = Response;
    type Body = Vec<u8>;

    async fn fetch(
        &self,
        url: &Url,
        method: &str,
        headers: &[(String, String)],
        body: Option<Self::Body>,
    ) -> Result<Response, Self::Error> {
        self.requests.borrow_mut().push(Request {
            method: method.into(),
            url: url.clone(),
            headers: headers.to_vec(),
            upload: body,
        });

        self.responses
            .borrow_mut()
            .pop_front()
            .expect("unexpected fetch")
    }
}

fn header(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

fn request(method: &str) -> Request {
    Request {
        method: method.into(),
        upload: None,
        url: Url::parse("https://proxy.example/github.com/a/b/releases/latest/download/f?x=%2F")
            .unwrap(),
        headers: vec![
            ("X-Holyproxy-Key".into(), "test-key".into()),
            ("Range".into(), "bytes=0-31".into()),
            ("Authorization".into(), "secret".into()),
            ("Cookie".into(), "session=secret".into()),
        ],
    }
}

fn redirect(location: &str) -> Response {
    Response::text(302, "", &[("Location", location)]).unwrap()
}

#[test]
fn rejects_invalid_requests_without_fetching() {
    let client = MockClient::default();
    let error = block_on(proxy(
        &client,
        &AllowedKeys,
        &MockAudit::default(),
        request("POST"),
    ))
    .unwrap_err();

    assert_eq!(error, ProxyError::UnsupportedMethod("GET, HEAD"));

    let mut invalid = request("GET");
    invalid.url = Url::parse("https://proxy.example/https://example.com/f").unwrap();

    assert_eq!(
        block_on(proxy(&client, &AllowedKeys, &MockAudit::default(), invalid)).unwrap_err(),
        ProxyError::InvalidTarget
    );
    assert!(client.requests.borrow().is_empty());
}

#[test]
fn follows_redirects_and_preserves_download_responses() {
    for method in ["GET", "HEAD"] {
        let client = MockClient::default();
        let body = if method == "GET" {
            "download bytes"
        } else {
            ""
        };
        let final_response =
            Response::text(206, body, &[("Content-Range", "bytes 0-31/100")]).unwrap();
        let body_ptr = final_response.body.0.as_ptr();

        client.responses.borrow_mut().extend([
            Ok(redirect("/a/b/releases/download/v1/f")),
            Ok(redirect(
                "https://release-assets.githubusercontent.com/file?sig=%2F",
            )),
            Ok(final_response),
        ]);

        let response = block_on(proxy(
            &client,
            &AllowedKeys,
            &MockAudit::default(),
            request(method),
        ))
        .unwrap();

        assert_eq!(response.status, 206);
        assert_eq!(
            response.header("Content-Range").unwrap().as_deref(),
            Some("bytes 0-31/100")
        );
        assert_eq!(&*response.body.0, body);
        assert_eq!(response.body.0.as_ptr(), body_ptr);

        let requests = client.requests.borrow();

        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].url.query(), Some("x=%2F"));
        assert_eq!(requests[2].url.query(), Some("sig=%2F"));

        for sent in requests.iter() {
            assert_eq!(sent.method, method);
            assert_eq!(sent.header("Range").unwrap().as_deref(), Some("bytes=0-31"));
            assert_eq!(
                sent.header("User-Agent").unwrap().as_deref(),
                Some("holyproxy")
            );
            assert!(sent.header("X-Holyproxy-Key").unwrap().is_none());
            assert!(sent.header("Authorization").unwrap().is_none());
            assert!(sent.header("Cookie").unwrap().is_none());
        }
    }
}

#[test]
fn preserves_upstream_error_responses() {
    for status in [404, 429, 500] {
        let client = MockClient::default();
        let upstream = Response::text(status, "upstream error", &[("Retry-After", "60")]).unwrap();
        let body_ptr = upstream.body.0.as_ptr();
        client.responses.borrow_mut().push_back(Ok(upstream));

        let response = block_on(proxy(
            &client,
            &AllowedKeys,
            &MockAudit::default(),
            request("GET"),
        ))
        .unwrap();

        assert_eq!(response.status, status);
        assert_eq!(
            response.header("Retry-After").unwrap().as_deref(),
            Some("60")
        );
        assert_eq!(response.body.0.as_ptr(), body_ptr);
    }
}

#[test]
fn rejects_invalid_or_missing_redirects() {
    for location in [Some("https://example.com/file"), None] {
        let client = MockClient::default();
        let mut response = redirect(location.unwrap_or_default());

        if location.is_none() {
            response.headers.clear();
        }

        client.responses.borrow_mut().push_back(Ok(response));

        assert_eq!(
            block_on(proxy(
                &client,
                &AllowedKeys,
                &MockAudit::default(),
                request("GET")
            ))
            .unwrap_err(),
            ProxyError::InvalidRedirect
        );
        assert_eq!(client.requests.borrow().len(), 1);
    }
}

#[test]
fn limits_redirects() {
    let client = MockClient::default();
    client
        .responses
        .borrow_mut()
        .extend((0..11).map(|_| Ok(redirect("/a/b/releases/download/v1/f"))));

    assert_eq!(
        block_on(proxy(
            &client,
            &AllowedKeys,
            &MockAudit::default(),
            request("GET")
        ))
        .unwrap_err(),
        ProxyError::InvalidRedirect
    );
    assert_eq!(client.requests.borrow().len(), 11);
}

#[test]
fn retains_runtime_errors_for_the_adapter() {
    let client = MockClient::default();
    client
        .responses
        .borrow_mut()
        .push_back(Err("network failure"));

    assert_eq!(
        block_on(proxy(
            &client,
            &AllowedKeys,
            &MockAudit::default(),
            request("GET")
        ))
        .unwrap_err(),
        ProxyError::Runtime("network failure")
    );
}

#[test]
fn maps_errors_to_public_responses() {
    for (error, status, message) in [
        (ProxyError::Forbidden, 403, "Forbidden"),
        (
            ProxyError::AuthStore("private details"),
            503,
            "Authorization service unavailable",
        ),
        (
            ProxyError::UnsupportedMethod("GET, HEAD"),
            405,
            "Method not allowed for this endpoint",
        ),
        (
            ProxyError::InvalidTarget,
            400,
            "Expected a GitHub Release attachment or Git Smart HTTP URL",
        ),
        (
            ProxyError::InvalidRedirect,
            502,
            "Invalid upstream redirect or too many redirects",
        ),
        (
            ProxyError::Runtime("private runtime details"),
            502,
            "Upstream request failed",
        ),
    ] {
        let response = error_response::<Response>(error).unwrap();

        assert_eq!(response.status, status);
        assert_eq!(&*response.body.0, message);
        assert_eq!(
            response.header("Allow").unwrap().as_deref(),
            if status == 405 {
                Some("GET, HEAD")
            } else {
                None
            }
        );
    }
}

struct AllowedKeys;

impl ApiKeyStore for AllowedKeys {
    type Error = &'static str;

    async fn contains(&self, _: &str) -> Result<Option<ApiKeyRecord>, Self::Error> {
        Ok(Some(ApiKeyRecord {
            username: "alice".into(),
        }))
    }
}

struct MockKeys {
    result: Result<Option<ApiKeyRecord>, &'static str>,
    hashes: RefCell<Vec<String>>,
}

impl ApiKeyStore for MockKeys {
    type Error = &'static str;

    async fn contains(&self, hash: &str) -> Result<Option<ApiKeyRecord>, Self::Error> {
        self.hashes.borrow_mut().push(hash.into());
        self.result.clone()
    }
}

#[test]
fn authorizes_each_transport_once_before_redirects() {
    for (mask, method) in (1..8).flat_map(|mask| ["GET", "HEAD"].map(|method| (mask, method))) {
        let (header, query, basic) = (mask & 1 != 0, mask & 2 != 0, mask & 4 != 0);
        let keys = MockKeys {
            result: Ok(Some(ApiKeyRecord {
                username: "alice".into(),
            })),
            hashes: RefCell::default(),
        };
        let client = MockClient::default();
        client.responses.borrow_mut().extend([
            Ok(redirect(
                "https://release-assets.githubusercontent.com/f?sig=%2F",
            )),
            Ok(Response::text(200, "download", &[]).unwrap()),
        ]);
        let mut req = request(method);
        if basic {
            set_basic(&mut req, "abc");
        }
        req.headers.retain(|(name, _)| name != "X-Holyproxy-Key");
        if header {
            req.headers.push(("x-holyproxy-key".into(), "abc".into()));
        }
        if query {
            req.url.set_query(Some("x=%2F&x_holyproxy_key=abc&x=+"));
        }
        let audit = MockAudit::default();
        block_on(proxy(&client, &keys, &audit, req)).unwrap();
        assert_eq!(audit.0.borrow()[0].username.as_deref(), Some("alice"));
        assert_eq!(
            &*keys.hashes.borrow(),
            &["ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"]
        );
        let sent = client.requests.borrow();
        assert_eq!(
            sent[0].url.query(),
            Some(if query { "x=%2F&x=+" } else { "x=%2F" })
        );
        assert_eq!(sent[1].url.query(), Some("sig=%2F"));
        for req in sent.iter() {
            assert!(req.header("X-Holyproxy-Key").unwrap().is_none());
            assert!(req.header("Authorization").unwrap().is_none());
            assert_eq!(req.method, method);
            assert_eq!(req.header("Range").unwrap().as_deref(), Some("bytes=0-31"));
        }
    }
}

#[test]
fn rejects_credentials_without_fetching_or_unnecessary_lookup() {
    for (header, query) in [
        (None, None),
        (Some(""), None),
        (Some("a, b"), None),
        (None, Some("x_holyproxy_key=")),
        (None, Some("x_holyproxy_key=a&x_holyproxy_key=a")),
        (Some("a"), Some("x_holyproxy_key=b")),
    ] {
        let keys = MockKeys {
            result: Ok(Some(ApiKeyRecord {
                username: "alice".into(),
            })),
            hashes: RefCell::default(),
        };
        let client = MockClient::default();
        let mut req = request("GET");
        req.headers.retain(|(name, _)| name != "X-Holyproxy-Key");
        if let Some(header) = header {
            req.headers.push(("X-Holyproxy-Key".into(), header.into()));
        }
        req.url.set_query(query);
        assert_eq!(
            block_on(proxy(&client, &keys, &MockAudit::default(), req)).unwrap_err(),
            ProxyError::Forbidden
        );
        assert!(keys.hashes.borrow().is_empty());
        assert!(client.requests.borrow().is_empty());
    }
    for (result, expected) in [
        (Ok(None), ProxyError::Forbidden),
        (
            Err("KV unavailable"),
            ProxyError::AuthStore("KV unavailable"),
        ),
    ] {
        let keys = MockKeys {
            result,
            hashes: RefCell::default(),
        };
        let client = MockClient::default();
        assert_eq!(
            block_on(proxy(&client, &keys, &MockAudit::default(), request("GET"))).unwrap_err(),
            expected
        );
        assert_eq!(keys.hashes.borrow().len(), 1);
        assert!(client.requests.borrow().is_empty());
    }
}

#[derive(Default)]
struct MockAudit(RefCell<Vec<AuditEvent>>);

impl Audit for MockAudit {
    fn record(&self, event: AuditEvent) {
        self.0.borrow_mut().push(event);
    }
}

#[test]
fn audits_final_responses_once_without_consuming_bodies() {
    for method in ["GET", "HEAD"] {
        for status in [200, 206, 304, 404, 429, 500] {
            let client = MockClient::default();
            let audit = MockAudit::default();
            let response = Response::text(status, "opaque stream", &[]).unwrap();
            let body_ptr = response.body.0.as_ptr();
            client.responses.borrow_mut().extend([
                Ok(redirect("/a/b/releases/download/v1/f")),
                Ok(redirect("https://release-assets.githubusercontent.com/f")),
                Ok(response),
            ]);
            let response = block_on(proxy(&client, &AllowedKeys, &audit, request(method))).unwrap();
            assert_eq!(response.status, status);
            assert_eq!(response.body.0.as_ptr(), body_ptr);
            assert_eq!(client.requests.borrow().len(), 3);
            assert_eq!(
                &*audit.0.borrow(),
                &[AuditEvent {
                    username: Some("alice".into()),
                    outcome: if status < 400 {
                        AuditOutcome::Success
                    } else {
                        AuditOutcome::Failure
                    },
                }]
            );
        }
    }
}

#[test]
fn audits_authenticated_errors_and_unauthenticated_requests() {
    for case in [
        "missing",
        "conflicting",
        "unknown",
        "store",
        "method",
        "target",
        "network",
        "redirect",
        "limit",
    ] {
        let client = MockClient::default();
        let audit = MockAudit::default();
        let mut req = request("GET");
        let mut keys = MockKeys {
            result: Ok(Some(ApiKeyRecord {
                username: "alice".into(),
            })),
            hashes: RefCell::default(),
        };
        let (expected, username) = match case {
            "missing" => {
                req.headers.clear();
                (ProxyError::Forbidden, None)
            }
            "conflicting" => {
                req.url.set_query(Some("x_holyproxy_key=other"));
                (ProxyError::Forbidden, None)
            }
            "unknown" => {
                keys.result = Ok(None);
                (ProxyError::Forbidden, None)
            }
            "store" => {
                keys.result = Err("unavailable");
                (ProxyError::AuthStore("unavailable"), None)
            }
            "method" => {
                req.method = "POST".into();
                (ProxyError::UnsupportedMethod("GET, HEAD"), Some("alice"))
            }
            "target" => {
                req.url.set_path("/invalid");
                (ProxyError::InvalidTarget, Some("alice"))
            }
            "network" => {
                client.responses.borrow_mut().push_back(Err("network"));
                (ProxyError::Runtime("network"), Some("alice"))
            }
            "redirect" => {
                client
                    .responses
                    .borrow_mut()
                    .push_back(Ok(redirect("https://evil.example/f")));
                (ProxyError::InvalidRedirect, Some("alice"))
            }
            "limit" => {
                client
                    .responses
                    .borrow_mut()
                    .extend((0..11).map(|_| Ok(redirect("/a/b/releases/download/v1/f"))));
                (ProxyError::InvalidRedirect, Some("alice"))
            }
            _ => unreachable!(),
        };
        assert_eq!(
            block_on(proxy(&client, &keys, &audit, req)).unwrap_err(),
            expected
        );
        assert_eq!(
            &*audit.0.borrow(),
            &[AuditEvent {
                username: username.map(str::to_owned),
                outcome: AuditOutcome::Failure,
            }],
            "{case}"
        );
    }
}

#[test]
fn audits_request_metadata_errors_as_anonymous() {
    struct BrokenRequest(bool);
    impl ProxyRequest for BrokenRequest {
        type Error = &'static str;
        type Body = Vec<u8>;
        fn take_body(&mut self) -> Result<Option<Self::Body>, Self::Error> {
            panic!("must not read body")
        }
        fn method(&self) -> String {
            "GET".into()
        }
        fn url(&self) -> Result<Url, Self::Error> {
            if self.0 {
                Err("metadata")
            } else {
                Ok(request("GET").url)
            }
        }
        fn header(&self, _: &str) -> Result<Option<String>, Self::Error> {
            Err("metadata")
        }
    }
    for url_error in [false, true] {
        let audit = MockAudit::default();
        let client = MockClient::default();
        assert_eq!(
            block_on(crate::handle_request(
                &client,
                &AllowedKeys,
                &audit,
                BrokenRequest(url_error)
            ))
            .unwrap_err(),
            ProxyError::Runtime("metadata")
        );
        assert_eq!(
            &*audit.0.borrow(),
            &[AuditEvent {
                username: None,
                outcome: AuditOutcome::Failure
            }]
        );
        assert!(client.requests.borrow().is_empty());
    }
}

fn set_basic(req: &mut Request, key: &str) {
    req.headers
        .retain(|(name, _)| !name.eq_ignore_ascii_case("Authorization"));
    req.headers.push((
        "Authorization".into(),
        format!(
            "Basic {}",
            STANDARD.encode(format!("x_holyproxy_key:{key}"))
        ),
    ));
}

#[test]
fn basic_failures_do_not_fetch() {
    for (authorization, result, lookups, expected) in [
        ("Basic !!!".to_owned(), Ok(None), 0, ProxyError::Forbidden),
        (
            format!("Basic {}", STANDARD.encode("x_holyproxy_key:other")),
            Ok(None),
            0,
            ProxyError::Forbidden,
        ),
        (
            format!("Basic {}", STANDARD.encode("x_holyproxy_key:test-key")),
            Ok(None),
            1,
            ProxyError::Forbidden,
        ),
        (
            format!("Basic {}", STANDARD.encode("x_holyproxy_key:test-key")),
            Err("D1 unavailable"),
            1,
            ProxyError::AuthStore("D1 unavailable"),
        ),
    ] {
        let keys = MockKeys {
            result,
            hashes: RefCell::default(),
        };
        let client = MockClient::default();
        let audit = MockAudit::default();
        let mut req = request("GET");
        req.headers.retain(|(name, _)| name != "Authorization");
        req.headers.push(("Authorization".into(), authorization));
        assert_eq!(
            block_on(proxy(&client, &keys, &audit, req)).unwrap_err(),
            expected
        );
        assert_eq!(keys.hashes.borrow().len(), lookups);
        assert!(client.requests.borrow().is_empty());
        assert_eq!(audit.0.borrow()[0].username, None);
        assert_eq!(audit.0.borrow()[0].outcome, AuditOutcome::Failure);
    }
}

fn git_request(method: &str, suffix: &str) -> Request {
    let mut req = request(method);
    req.url = Url::parse(&format!(
        "https://proxy.example/https://github.com/a/b.git/{suffix}"
    ))
    .unwrap();
    req.headers.retain(|(name, _)| name != "X-Holyproxy-Key");
    set_basic(&mut req, "test-key");
    req.headers.extend([
        ("Git-Protocol".into(), "version=2".into()),
        (
            "Content-Type".into(),
            "application/x-git-upload-pack-request; charset=binary".into(),
        ),
    ]);
    req
}

#[test]
fn git_upload_moves_body_and_preserves_response() {
    let client = MockClient::default();
    client.responses.borrow_mut().push_back(Ok(Response::text(
        200,
        "pack",
        &[("Content-Type", "application/x-git-upload-pack-result")],
    )
    .unwrap()));
    let mut req = git_request("POST", "git-upload-pack");
    let bytes = vec![0, 255, 128, 13, 10];
    let ptr = bytes.as_ptr();
    req.upload = Some(bytes);
    let audit = MockAudit::default();
    let response = block_on(proxy(&client, &AllowedKeys, &audit, req)).unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(
        response.header("Content-Type").unwrap().as_deref(),
        Some("application/x-git-upload-pack-result")
    );
    let sent = client.requests.borrow();
    assert_eq!(sent[0].upload.as_ref().unwrap().as_ptr(), ptr);
    assert_eq!(
        sent[0].upload.as_deref(),
        Some([0, 255, 128, 13, 10].as_slice())
    );
    assert_eq!(
        sent[0].header("Git-Protocol").unwrap().as_deref(),
        Some("version=2")
    );
    for name in ["Authorization", "Cookie", "Range", "X-Holyproxy-Key"] {
        assert!(sent[0].header(name).unwrap().is_none());
    }
    assert_eq!(audit.0.borrow()[0].username.as_deref(), Some("alice"));
}

#[test]
fn git_method_and_media_type_errors_do_not_fetch() {
    for (method, suffix, expected) in [
        (
            "POST",
            "info/refs?service=git-upload-pack",
            ProxyError::UnsupportedMethod("GET"),
        ),
        (
            "HEAD",
            "info/refs?service=git-upload-pack",
            ProxyError::UnsupportedMethod("GET"),
        ),
        (
            "GET",
            "git-upload-pack",
            ProxyError::UnsupportedMethod("POST"),
        ),
        ("POST", "git-upload-pack", ProxyError::UnsupportedMediaType),
    ] {
        let client = MockClient::default();
        let mut req = git_request(method, suffix);
        req.headers.retain(|(name, _)| name != "Content-Type");
        let error = block_on(proxy(&client, &AllowedKeys, &MockAudit::default(), req)).unwrap_err();
        assert_eq!(error, expected);
        let response = error_response::<Response>(error).unwrap();
        if let ProxyError::UnsupportedMethod(allow) = expected {
            assert_eq!(response.header("Allow").unwrap().as_deref(), Some(allow));
        } else {
            assert_eq!(response.status, 415);
        }
        assert!(client.requests.borrow().is_empty());
    }
}

#[test]
fn git_discovery_redirects_strip_credentials_and_post_never_replays() {
    let client = MockClient::default();
    client.responses.borrow_mut().extend([
        Ok(redirect(
            "/other/repo.git/info/refs?service=git-upload-pack",
        )),
        Ok(Response::text(200, "refs", &[]).unwrap()),
    ]);
    let req = git_request(
        "GET",
        "info/refs?service=git-upload-pack&x_holyproxy_key=test-key",
    );
    block_on(proxy(&client, &AllowedKeys, &MockAudit::default(), req)).unwrap();
    for sent in client.requests.borrow().iter() {
        assert_eq!(sent.url.query(), Some("service=git-upload-pack"));
        assert!(sent.upload.is_none());
        assert!(sent.header("Authorization").unwrap().is_none());
    }
    for status in [301, 302, 303, 307, 308] {
        let client = MockClient::default();
        client.responses.borrow_mut().push_back(Ok(Response::text(
            status,
            "",
            &[("Location", "/other/repo.git/git-upload-pack")],
        )
        .unwrap()));
        assert_eq!(
            block_on(proxy(
                &client,
                &AllowedKeys,
                &MockAudit::default(),
                git_request("POST", "git-upload-pack")
            ))
            .unwrap_err(),
            ProxyError::InvalidRedirect
        );
        assert_eq!(client.requests.borrow().len(), 1);
    }
}

#[test]
fn git_discovery_rejects_missing_unsafe_and_looping_redirects() {
    for location in [
        None,
        Some("https://evil.example/a/b.git/info/refs?service=git-upload-pack"),
        Some("/a/b.git/info/refs?service=git-upload-pack"),
    ] {
        let client = MockClient::default();
        let looping = location.is_some_and(|value| value.starts_with('/'));
        let count = if looping { 11 } else { 1 };
        for _ in 0..count {
            client.responses.borrow_mut().push_back(Ok(Response::text(
                302,
                "",
                &location
                    .map(|value| vec![("Location", value)])
                    .unwrap_or_default(),
            )
            .unwrap()));
        }
        assert_eq!(
            block_on(proxy(
                &client,
                &AllowedKeys,
                &MockAudit::default(),
                git_request("GET", "info/refs?service=git-upload-pack")
            ))
            .unwrap_err(),
            ProxyError::InvalidRedirect
        );
        assert_eq!(client.requests.borrow().len(), count);
    }
}

#[test]
fn git_missing_credentials_challenges_without_lookup_or_fetch() {
    for (method, suffix) in [
        ("GET", "info/refs?service=git-upload-pack"),
        ("POST", "git-upload-pack"),
    ] {
        let keys = MockKeys {
            result: Err("must not query"),
            hashes: RefCell::default(),
        };
        let client = MockClient::default();
        let audit = MockAudit::default();
        let mut req = git_request(method, suffix);
        req.headers.clear();
        let error = block_on(proxy(&client, &keys, &audit, req)).unwrap_err();
        assert_eq!(error, ProxyError::AuthenticationRequired);
        let response = error_response::<Response>(error).unwrap();
        assert_eq!(response.status, 401);
        assert_eq!(
            response.header("WWW-Authenticate").unwrap().as_deref(),
            Some("Basic realm=\"holyproxy\"")
        );
        assert_eq!(
            response.header("Cache-Control").unwrap().as_deref(),
            Some("no-store")
        );
        assert!(keys.hashes.borrow().is_empty());
        assert!(client.requests.borrow().is_empty());
        assert_eq!(
            &*audit.0.borrow(),
            &[AuditEvent {
                username: None,
                outcome: AuditOutcome::Failure
            }]
        );
    }
}

#[test]
fn git_invalid_credentials_and_other_routes_do_not_challenge() {
    for (suffix, headers) in [
        (
            "info/refs?service=git-upload-pack",
            vec![("Authorization", "Basic !!!")],
        ),
        (
            "info/refs?service=git-upload-pack",
            vec![("Authorization", "Bearer other")],
        ),
        (
            "info/refs?service=git-upload-pack",
            vec![("X-Holyproxy-Key", "")],
        ),
        (
            "info/refs?service=git-upload-pack&%78_holyproxy_key=",
            vec![],
        ),
        ("info/refs?service=git-receive-pack", vec![]),
        ("git-receive-pack", vec![]),
    ] {
        let client = MockClient::default();
        let keys = MockKeys {
            result: Err("must not query"),
            hashes: RefCell::default(),
        };
        let mut req = git_request("GET", suffix);
        req.headers = headers
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        let error = block_on(proxy(&client, &keys, &MockAudit::default(), req)).unwrap_err();
        assert_eq!(error, ProxyError::Forbidden);
        assert!(
            error_response::<Response>(error)
                .unwrap()
                .header("WWW-Authenticate")
                .unwrap()
                .is_none()
        );
        assert!(keys.hashes.borrow().is_empty());
        assert!(client.requests.borrow().is_empty());
    }
    let mut req = request("GET");
    req.headers.clear();
    assert_eq!(
        block_on(proxy(
            &MockClient::default(),
            &AllowedKeys,
            &MockAudit::default(),
            req
        ))
        .unwrap_err(),
        ProxyError::Forbidden
    );
}
