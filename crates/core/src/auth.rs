use sha2::{Digest, Sha256};
use std::future::Future;
use url::form_urlencoded;

/// Identity associated with an allowlisted API key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiKeyRecord {
    pub username: String,
}

/// External allowlist lookup. Digests are lowercase SHA-256 hex, without a prefix.
pub trait ApiKeyStore {
    type Error;

    fn contains(
        &self,
        key_hash: &str,
    ) -> impl Future<Output = Result<Option<ApiKeyRecord>, Self::Error>>;
}

/// Extract credentials while retaining the original bytes of unrelated query fields.
pub(crate) fn credentials(
    header: Option<&str>,
    query: Option<&str>,
) -> Option<(String, Option<String>)> {
    let mut query_key = None;
    let mut retained = Vec::new();
    if let Some(query) = query {
        for field in query.split('&') {
            let pair = form_urlencoded::parse(field.as_bytes()).next();
            if let Some((_, value)) = pair.filter(|(name, _)| name == "x_holyproxy_key") {
                if query_key.replace(value.into_owned()).is_some() {
                    return None;
                }
            } else {
                retained.push(field);
            }
        }
    }
    let key = match (header, query_key.as_deref()) {
        (Some(header), Some(query)) if header == query => header,
        (Some(header), None) => header,
        (None, Some(query)) => query,
        _ => return None,
    };
    if key.is_empty()
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return None;
    }
    let digest = format!("{:x}", Sha256::digest(key.as_bytes()));
    let query = if retained.is_empty() {
        None
    } else {
        Some(retained.join("&"))
    };
    Some((digest, query))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_decoded_key_and_preserves_unrelated_query_bytes() {
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        for (query, retained) in [
            ("x_holyproxy_key=abc", None),
            ("x_holyproxy_key=abc&a=%2f&a=+&flag", Some("a=%2f&a=+&flag")),
            ("a=%2F&x_holyproxy_key=abc&a=%20", Some("a=%2F&a=%20")),
            ("a=1&x_holyproxy_key=abc", Some("a=1")),
            ("%78_holyproxy_key=%61bc&x=1", Some("x=1")),
            ("&x_holyproxy_key=abc&&x=1&", Some("&&x=1&")),
        ] {
            let (hash, cleaned) = credentials(None, Some(query)).unwrap();
            assert_eq!(hash, expected);
            assert_eq!(cleaned.as_deref(), retained);
        }
        assert_eq!(
            credentials(Some("abc"), None).unwrap(),
            (expected.into(), None)
        );
        assert_eq!(
            credentials(Some("abc"), Some("")).unwrap().1.as_deref(),
            Some("")
        );
    }

    #[test]
    fn rejects_ambiguous_or_invalid_credentials() {
        for (header, query) in [
            (None, None),
            (Some(""), None),
            (None, Some("x_holyproxy_key")),
            (Some("abc"), Some("x_holyproxy_key=def")),
            (Some("abc"), Some("x_holyproxy_key=")),
            (None, Some("x_holyproxy_key=abc&%78_holyproxy_key=abc")),
            (Some("abc, abc"), None),
            (Some(" abc"), None),
            (Some("abc\n"), None),
            (Some("密钥"), None),
            (None, Some("x_holyproxy_key=a+b")),
            (None, Some("x_holyproxy_key=%FF")),
            (None, Some("x_holyproxy_key=%ZZ")),
        ] {
            assert!(credentials(header, query).is_none());
        }
        assert!(credentials(Some("A_b-9"), Some("x_holyproxy_key=A_b-9")).is_some());
    }
}
