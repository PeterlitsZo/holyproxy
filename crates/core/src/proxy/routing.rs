use regex::Regex;
use std::sync::LazyLock;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum TargetKind {
    Release,
    GitDiscovery,
    GitUploadPack,
}

impl TargetKind {
    pub fn allowed_methods(self) -> &'static str {
        match self {
            Self::Release => "GET, HEAD",
            Self::GitDiscovery => "GET",
            Self::GitUploadPack => "POST",
        }
    }
}

pub(super) fn target_url(path: &str, query: Option<&str>) -> Option<(Url, TargetKind)> {
    if let Some(url) = release_url(path, query) {
        return Some((url, TargetKind::Release));
    }
    let target = path.strip_prefix('/')?;
    let target = target.strip_prefix("https://").unwrap_or(target);
    let mut url = Url::parse(&format!("https://{target}")).ok()?;
    if !secure_url(&url) || url.host_str() != Some("github.com") {
        return None;
    }
    // GitHub repository names need no escaped separators or dot segments.
    static GIT_PATH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\A/[A-Za-z0-9_-]+/[A-Za-z0-9_.-]+\.git/(info/refs|git-upload-pack)\z")
            .expect("valid Git path regex")
    });
    if !GIT_PATH.is_match(url.path()) || url.path() != &target[target.find('/')?..] {
        return None;
    }
    url.set_query(query);
    let kind = if url.path().ends_with("/info/refs") {
        let pairs: Vec<_> = url.query_pairs().collect();
        if pairs.len() != 1 || pairs[0].0 != "service" || pairs[0].1 != "git-upload-pack" {
            return None;
        }
        TargetKind::GitDiscovery
    } else {
        if query.is_some_and(|q| !q.is_empty()) {
            return None;
        }
        TargetKind::GitUploadPack
    };
    Some((url, kind))
}

pub(super) fn git_redirect_url(current: &Url, location: &str) -> Option<Url> {
    let next = current.join(location).ok()?;
    if !secure_url(&next) || next.host_str() != Some("github.com") {
        return None;
    }
    let (url, kind) = target_url(&format!("/https://github.com{}", next.path()), next.query())?;
    (kind == TargetKind::GitDiscovery).then_some(url)
}

/// Parse the proxy path without decoding escaped tags, filenames or query
/// strings.
pub(super) fn release_url(path: &str, query: Option<&str>) -> Option<Url> {
    let target = path.strip_prefix('/')?;
    let target = target.strip_prefix("https://").unwrap_or(target);
    let mut url = Url::parse(&format!("https://{target}")).ok()?;

    if !secure_url(&url) || url.host_str() != Some("github.com") {
        return None;
    }

    if !is_release_path(url.path()) {
        return None;
    }

    url.set_query(query);
    Some(url)
}

/// Check whether a URL path identifies a versioned or latest Release
/// attachment.
fn is_release_path(path: &str) -> bool {
    static RELEASE_PATH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\A/[^/]+/[^/]+/releases/(?:download/[^/]+|latest/download)/[^/]+\z")
            .expect("valid Release path regex")
    });

    RELEASE_PATH.is_match(path)
}

fn secure_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.fragment().is_none()
}

/// Resolve only GitHub's Release download redirects (including relative URLs).
pub(super) fn redirect_url(current: &Url, location: &str) -> Option<Url> {
    let next = current.join(location).ok()?;

    if !secure_url(&next) {
        return None;
    }

    match next.host_str()? {
        "github.com" => release_url(&format!("/https://github.com{}", next.path()), next.query()),
        "release-assets.githubusercontent.com"
        | "objects.githubusercontent.com"
        | "github-releases.githubusercontent.com" => Some(next),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_release_links_and_preserves_encoding() {
        for prefix in ["/https://github.com", "/github.com"] {
            let url = release_url(
                &format!("{prefix}/a/b/releases/download/v1%2F2/a%20b.zip"),
                Some("x=%2F&x=2"),
            )
            .unwrap();

            assert_eq!(
                url.as_str(),
                "https://github.com/a/b/releases/download/v1%2F2/a%20b.zip?x=%2F&x=2"
            );

            assert!(
                release_url(
                    &format!("{prefix}/a/b/releases/latest/download/file.zip"),
                    None
                )
                .is_some()
            );
        }
    }

    #[test]
    fn rejects_other_targets() {
        for target in [
            "https://evil.com/a/b/releases/download/v/f",
            "https://github.com.evil.com/a/b/releases/download/v/f",
            "https://user@github.com/a/b/releases/download/v/f",
            "https://github.com:8443/a/b/releases/download/v/f",
            "http://github.com/a/b/releases/download/v/f",
            "https://github.com/a/b/archive/main.zip",
            "https://github.com/a/b/releases/tag/v1",
            "https://github.com/a/b/releases/download/v/",
            "https://github.com/a/b/releases/download/v/f/../../../../issues",
        ] {
            assert!(
                release_url(&format!("/{target}"), None).is_none(),
                "{target}"
            );
        }
    }

    #[test]
    fn resolves_download_redirects_only() {
        let current = release_url("/github.com/a/b/releases/latest/download/f", None).unwrap();
        assert!(redirect_url(&current, "/a/b/releases/download/v1/f").is_some());

        let signed = "https://release-assets.githubusercontent.com/path?sig=a%2Fb";
        assert_eq!(redirect_url(&current, signed).unwrap().as_str(), signed);

        for target in [
            "https://evil.com/f",
            "http://objects.githubusercontent.com/f",
            "https://github.com/login",
            "https://objects.githubusercontent.com.evil.com/f",
        ] {
            assert!(redirect_url(&current, target).is_none());
        }
    }
}

#[cfg(test)]
mod git_tests {
    use super::*;

    #[test]
    fn git_targets_and_query_rules() {
        for prefix in ["/github.com", "/https://github.com"] {
            assert_eq!(
                target_url(
                    &format!("{prefix}/a/b.git/info/refs"),
                    Some("service=git-upload-pack")
                )
                .unwrap()
                .1,
                TargetKind::GitDiscovery
            );
            assert_eq!(
                target_url(&format!("{prefix}/a/b.git/git-upload-pack"), None)
                    .unwrap()
                    .1,
                TargetKind::GitUploadPack
            );
        }
        for query in [
            None,
            Some(""),
            Some("service=git-receive-pack"),
            Some("service=git-upload-pack&service=git-upload-pack"),
            Some("service=git-upload-pack&other=1"),
        ] {
            assert!(target_url("/github.com/a/b.git/info/refs", query).is_none());
        }
        assert!(
            target_url(
                "/github.com/a/b.git/git-upload-pack",
                Some("service=git-upload-pack")
            )
            .is_none()
        );
        for path in [
            "/github.com/a/b.git/git-receive-pack",
            "/github.com/a/b/info/refs",
            "/github.com//b.git/git-upload-pack",
            "/github.com/a/.git/git-upload-pack",
            "/github.com/a/b%2fc.git/git-upload-pack",
            "/github.com/a/b%5cc.git/git-upload-pack",
            "/github.com/a/../b.git/git-upload-pack",
            "/github.com/a/%2e%2e/b.git/git-upload-pack",
            "/https://github.com.evil.example/a/b.git/git-upload-pack",
            "/https://user@github.com/a/b.git/git-upload-pack",
            "/https://github.com:8443/a/b.git/git-upload-pack",
            "/http://github.com/a/b.git/git-upload-pack",
        ] {
            assert!(target_url(path, None).is_none(), "{path}");
        }
    }

    #[test]
    fn git_redirects_stay_on_discovery_endpoint() {
        let current =
            Url::parse("https://github.com/a/b.git/info/refs?service=git-upload-pack").unwrap();
        assert!(git_redirect_url(&current, "/c/d.git/info/refs?service=git-upload-pack").is_some());
        for location in [
            "https://evil.example/a/b.git/info/refs?service=git-upload-pack",
            "http://github.com/a/b.git/info/refs?service=git-upload-pack",
            "/login",
            "/a/b.git/git-upload-pack",
            "/a/b.git/info/refs?service=git-receive-pack",
            "/a/b.git/info/refs?service=git-upload-pack#fragment",
        ] {
            assert!(git_redirect_url(&current, location).is_none(), "{location}");
        }
    }
}
