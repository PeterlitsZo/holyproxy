use super::*;
use crate::ApiKeyRecord;
use futures::executor::block_on;
use std::cell::RefCell;
use std::collections::VecDeque;

struct Request {
    method: String,
    url: Url,
    headers: Vec<(String, String)>,
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

    async fn fetch(
        &self,
        url: &Url,
        method: &str,
        headers: &[(String, String)],
    ) -> Result<Response, Self::Error> {
        self.requests.borrow_mut().push(Request {
            method: method.into(),
            url: url.clone(),
            headers: headers.to_vec(),
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

    assert_eq!(error, ProxyError::UnsupportedMethod);

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
            ProxyError::UnsupportedMethod,
            405,
            "Only GET and HEAD are supported",
        ),
        (
            ProxyError::InvalidTarget,
            400,
            "Expected a GitHub Release attachment URL",
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
    for (header, query) in [(true, false), (false, true), (true, true)] {
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
        let mut req = request("GET");
        req.headers.retain(|(name, _)| name != "X-Holyproxy-Key");
        if header {
            req.headers.push(("x-holyproxy-key".into(), "abc".into()));
        }
        if query {
            req.url.set_query(Some("x=%2F&x_holyproxy_key=abc&x=+"));
        }
        block_on(proxy(&client, &keys, &MockAudit::default(), req)).unwrap();
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
                (ProxyError::UnsupportedMethod, Some("alice"))
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
