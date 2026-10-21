//! Embedded homepage and platform-independent page response rules.

const HTML: &str = include_str!("../assets/index.html");

/// A static page response for a runtime adapter to send.
pub struct PageResponse {
    pub status: u16,
    pub headers: &'static [(&'static str, &'static str)],
    pub body: &'static str,
}

/// Respond to an already-matched root path without storage or network access.
pub fn index(method: &str) -> PageResponse {
    match method {
        "GET" | "HEAD" => PageResponse {
            status: 200,
            headers: &[
                ("Cache-Control", "no-store"),
                ("Referrer-Policy", "no-referrer"),
                ("X-Content-Type-Options", "nosniff"),
                ("Content-Type", "text/html; charset=utf-8"),
            ],
            body: if method == "HEAD" { "" } else { HTML },
        },
        _ => PageResponse {
            status: 405,
            headers: &[
                ("Cache-Control", "no-store"),
                ("Referrer-Policy", "no-referrer"),
                ("X-Content-Type-Options", "nosniff"),
                ("Allow", "GET, HEAD"),
            ],
            body: "Only GET and HEAD are supported",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_embedded_page_and_empty_head_with_identical_headers() {
        let get = index("GET");
        let head = index("HEAD");
        assert_eq!(get.status, 200);
        assert_eq!(head.status, 200);
        assert!(get.body.starts_with("<!doctype html>"));
        assert!(get.body.contains("<style>") && get.body.contains("<script>"));
        assert!(head.body.is_empty());
        assert_eq!(head.headers, get.headers);
        assert!(
            get.headers
                .contains(&("Content-Type", "text/html; charset=utf-8"))
        );
    }

    #[test]
    fn rejects_unsupported_methods_and_keeps_security_headers() {
        for method in ["GET", "HEAD", "POST", "OPTIONS", "DELETE", "PATCH"] {
            let page = index(method);
            for header in [
                ("Cache-Control", "no-store"),
                ("Referrer-Policy", "no-referrer"),
                ("X-Content-Type-Options", "nosniff"),
            ] {
                assert!(page.headers.contains(&header));
            }
            if !matches!(method, "GET" | "HEAD") {
                assert_eq!(page.status, 405);
                assert!(page.headers.contains(&("Allow", "GET, HEAD")));
                assert_eq!(page.body, "Only GET and HEAD are supported");
            }
        }
    }
}
