use holyproxy_core::{
    ApiKeyRecord, ApiKeyStore, HttpClient, ProxyError, ProxyRequest, ProxyResponse, error_response,
    proxy,
};
use worker::*;

mod audit;
use audit::WorkersAudit;

struct WorkersRequest(Request);
struct WorkersResponse(Response);
struct WorkersClient;
struct WorkersApiKeys(Env);

impl ApiKeyStore for WorkersApiKeys {
    type Error = Error;

    async fn contains(&self, key_hash: &str) -> Result<Option<ApiKeyRecord>> {
        #[derive(serde::Deserialize)]
        struct StoredApiKey {
            username: String,
        }

        let db = self.0.d1("HOLYPROXY")?;
        let record = db
            .prepare("SELECT username FROM api_keys WHERE key_hash = ?1")
            .bind(&[key_hash.into()])?
            .first::<StoredApiKey>(None)
            .await?;
        match record {
            Some(record) if record.username.trim().is_empty() => {
                Err(Error::RustError("Invalid API key identity".into()))
            }
            Some(record) => Ok(Some(ApiKeyRecord {
                username: record.username,
            })),
            None => Ok(None),
        }
    }
}

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
pub async fn main(req: Request, env: Env, ctx: Context) -> Result<Response> {
    let audit = WorkersAudit::new(env.clone(), ctx, Date::now().as_millis() / 1000);
    let response = match proxy(
        &WorkersClient,
        &WorkersApiKeys(env),
        &audit,
        WorkersRequest(req),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            if matches!(&error, ProxyError::AuthStore(_)) {
                console_error!("Release proxy authorization store failed");
            }
            if matches!(&error, ProxyError::Runtime(_)) {
                console_error!("Release proxy upstream request failed");
            }

            error_response::<WorkersResponse>(error)?
        }
    };

    Ok(response.0)
}
