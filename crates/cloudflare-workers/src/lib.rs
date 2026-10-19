use holyproxy_core::{HttpClient, ProxyError, ProxyRequest, ProxyResponse, error_response, proxy};
use worker::*;

struct WorkersRequest(Request);
struct WorkersResponse(Response);
struct WorkersClient;

impl ProxyRequest for WorkersRequest {
    type Error = Error;

    fn method(&self) -> String {
        self.0.method().to_string()
    }

    fn url(&self) -> Result<Url> {
        self.0.url()
    }

    fn header(&self, name: &str) -> Result<Option<String>> {
        self.0.headers().get(name)
    }
}

impl ProxyResponse for WorkersResponse {
    type Error = Error;

    fn status(&self) -> u16 {
        self.0.status_code()
    }

    fn header(&self, name: &str) -> Result<Option<String>> {
        self.0.headers().get(name)
    }

    fn text(status: u16, body: &str, headers: &[(&str, &str)]) -> Result<Self> {
        let mut response = Response::ok(body)?.with_status(status);

        for (name, value) in headers {
            response.headers_mut().set(name, value)?;
        }

        Ok(Self(response))
    }
}

impl HttpClient for WorkersClient {
    type Error = Error;
    type Response = WorkersResponse;

    async fn fetch(
        &self,
        url: &Url,
        method: &str,
        headers: &[(String, String)],
    ) -> Result<WorkersResponse> {
        let upstream_headers = Headers::new();

        for (name, value) in headers {
            upstream_headers.set(name, value)?;
        }

        let mut init = RequestInit::new();
        init.with_method(Method::from(method.to_owned()))
            .with_headers(upstream_headers)
            .with_redirect(RequestRedirect::Manual);

        let request = Request::new_with_init(url.as_str(), &init)?;
        let response = Fetch::Request(request).send().await?;

        Ok(WorkersResponse(response))
    }
}

#[event(fetch)]
pub async fn main(req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    let response = match proxy(&WorkersClient, WorkersRequest(req)).await {
        Ok(response) => response,
        Err(error) => {
            if matches!(&error, ProxyError::Runtime(_)) {
                console_error!("Release proxy upstream request failed");
            }

            error_response::<WorkersResponse>(error)?
        }
    };

    Ok(response.0)
}
