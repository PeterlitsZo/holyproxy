use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
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

/// Extract credentials while retaining the original bytes of unrelated query
/// fields.
pub(crate) fn credentials(
    header: Option<&str>,
    query: Option<&str>,
    authorization: Option<&str>,
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
    let basic = basic_key(authorization).ok()?;
    let mut sources = [header, query_key.as_deref(), basic.as_deref()]
        .into_iter()
        .flatten();
    let key = sources.next()?;
    if sources.any(|other| other != key) {
        return None;
    }
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

/// Ignore other schemes, but never fall back from malformed Basic credentials.
fn basic_key(authorization: Option<&str>) -> Result<Option<String>, ()> {
    let Some(value) = authorization else {
        return Ok(None);
    };
    let value = value.trim_matches([' ', '\t']);
    // Inspect comma-separated schemes too: a merged Basic header must not be ignored.
    let is_basic = |part: &str| {
        part.trim_start_matches([' ', '\t'])
            .split(|c: char| c.is_ascii_whitespace())
            .next()
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("Basic"))
    };
    if !value.split(',').any(is_basic) {
        return Ok(None);
    }
    let (scheme, encoded) = value.split_once(' ').ok_or(())?;
    if !scheme.eq_ignore_ascii_case("Basic") {
        return Err(());
    }
    let encoded = encoded.trim_start_matches(' ');
    // Separate strict engines accept either canonical padding or none, not partial padding.
    let decoded = if encoded.contains('=') {
        STANDARD.decode(encoded)
    } else {
        STANDARD_NO_PAD.decode(encoded)
    }
    .map_err(|_| ())?;
    let decoded = String::from_utf8(decoded).map_err(|_| ())?;
    let (username, password) = decoded.split_once(':').ok_or(())?;
    if username != "x_holyproxy_key" {
        return Err(());
    }
    Ok(Some(password.to_owned()))
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
            let (hash, cleaned) = credentials(None, Some(query), None).unwrap();
            assert_eq!(hash, expected);
            assert_eq!(cleaned.as_deref(), retained);
        }
        assert_eq!(
            credentials(Some("abc"), None, None).unwrap(),
            (expected.into(), None)
        );
        assert_eq!(
            credentials(Some("abc"), Some(""), None)
                .unwrap()
                .1
                .as_deref(),
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
            assert!(credentials(header, query, None).is_none());
        }
        assert!(credentials(Some("A_b-9"), Some("x_holyproxy_key=A_b-9"), None).is_some());
    }
    #[test]
    fn basic_variants_and_all_transport_combinations() {
        for key in ["a", "ab", "abc", "A_b-9"] {
            let encoded = STANDARD.encode(format!("x_holyproxy_key:{key}"));
            let query = format!("x_holyproxy_key={key}&x=%2f&x=+");
            for basic in [
                format!("Basic {encoded}"),
                format!("\t bAsIc   {} \t", encoded.trim_end_matches('=')),
            ] {
                for mask in 1..8 {
                    let result = credentials(
                        (mask & 1 != 0).then_some(key),
                        (mask & 2 != 0).then_some(query.as_str()),
                        (mask & 4 != 0).then_some(basic.as_str()),
                    )
                    .unwrap();
                    assert_eq!(result.0, credentials(Some(key), None, None).unwrap().0);
                    assert_eq!(result.1.as_deref(), (mask & 2 != 0).then_some("x=%2f&x=+"));
                }
            }
        }
        let basic = format!("Basic {}", STANDARD.encode("x_holyproxy_key:abc"));
        for (header, query) in [
            (Some("def"), None),
            (None, Some("x_holyproxy_key=def")),
            (Some("abc"), Some("x_holyproxy_key=def")),
            (Some("def"), Some("x_holyproxy_key=abc")),
            (Some("def"), Some("x_holyproxy_key=def")),
        ] {
            assert!(credentials(header, query, Some(&basic)).is_none());
        }
    }

    #[test]
    fn malformed_basic_never_falls_back() {
        let valid = STANDARD.encode("x_holyproxy_key:abc");
        let mut invalid = vec![
            "Basic".to_owned(),
            "Basic ".into(),
            "Basic !!!".into(),
            format!("Basic\t{valid}"),
            format!("Basic {valid} extra"),
            format!("Basic {valid}, Basic {valid}"),
            format!("Bearer other, Basic {valid}"),
            format!("Basic {}", valid.replace("==", "=")),
            format!("Basic {valid}="),
            format!("Basic {}", valid.replace("Yw==", "Yx==")),
            "Basic _w==".into(),
            format!("Basic {} {}", &valid[..4], &valid[4..]),
        ];
        for decoded in [
            b"wrong:abc".as_slice(),
            b"X_HOLYPROXY_KEY:abc",
            b"x_holyproxy_key:",
            b"x_holyproxy_key",
            b"x_holyproxy_key:abc:def",
            b"x_holyproxy_key:abc def",
            b"x_holyproxy_key:\xff",
        ] {
            invalid.push(format!("Basic {}", STANDARD.encode(decoded)));
        }
        for basic in invalid {
            for (header, query) in [
                (None, None),
                (Some("abc"), None),
                (None, Some("x_holyproxy_key=abc")),
                (Some("abc"), Some("x_holyproxy_key=abc")),
            ] {
                assert!(
                    credentials(header, query, Some(&basic)).is_none(),
                    "{basic}"
                );
            }
        }
        for other in ["secret", "Bearer abc"] {
            assert!(credentials(Some("abc"), None, Some(other)).is_some());
            assert!(credentials(None, None, Some(other)).is_none());
        }
    }
}
