use super::*;
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
    let error = block_on(proxy(&client, request("POST"))).unwrap_err();

    assert_eq!(error, ProxyError::UnsupportedMethod);

    let mut invalid = request("GET");
    invalid.url = Url::parse("https://proxy.example/https://example.com/f").unwrap();

    assert_eq!(
        block_on(proxy(&client, invalid)).unwrap_err(),
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

        let response = block_on(proxy(&client, request(method))).unwrap();

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

        let response = block_on(proxy(&client, request("GET"))).unwrap();

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
            block_on(proxy(&client, request("GET"))).unwrap_err(),
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
        block_on(proxy(&client, request("GET"))).unwrap_err(),
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
        block_on(proxy(&client, request("GET"))).unwrap_err(),
        ProxyError::Runtime("network failure")
    );
}

#[test]
fn maps_errors_to_public_responses() {
    for (error, status, message) in [
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
