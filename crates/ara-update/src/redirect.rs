//! Shared redirect and credential policy for metadata and streaming downloads.
use crate::{Error, Feed, NETWORK, URL};
use reqwest::{Url, header::HeaderValue};

pub(crate) const MAX_REDIRECTS: usize = 5;

/// Build credentials afresh for each hop; redirected asset hosts never receive them.
pub(crate) fn authorization(url: &Url, token: Option<&str>) -> Result<Option<HeaderValue>, Error> {
    if url.scheme() != "https"
        || url.host_str() != Some("api.github.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Ok(None);
    }
    let Some(token) = token else { return Ok(None) };
    let mut header = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| Error::new(NETWORK, "GitHub token cannot be used as an HTTP header"))?;
    header.set_sensitive(true);
    Ok(Some(header))
}

impl Feed {
    pub(crate) fn redirect(&self, current: &Url, location: &str) -> Result<Url, Error> {
        let target = self.url(
            current
                .join(location)
                .map_err(|_| Error::new(URL, "invalid release redirect"))?
                .as_str(),
        )?;
        #[cfg(any(test, feature = "test-support"))]
        if self.config.api.is_some() {
            return Ok(target);
        }
        // A moved repository redirects API paths to /repositories/<id>/..., and
        // existing signed browser URLs redirect to the new owner on github.com.
        let same_origin = target.origin() == current.origin()
            && matches!(target.host_str(), Some("api.github.com" | "github.com"));
        if !same_origin
            && !matches!(
                target.host_str(),
                Some(
                    "objects.githubusercontent.com"
                        | "release-assets.githubusercontent.com"
                        | "github-releases.githubusercontent.com"
                )
            )
        {
            return Err(Error::new(
                URL,
                "release redirect must stay on its GitHub API/download host or target an approved asset host",
            ));
        }
        Ok(target)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{
        Cancel, FeedConfig,
        testing::{self, Response, Server},
    };

    #[test]
    fn moved_api_and_signed_download_urls_keep_credentials_on_the_api_only() {
        let feed = Feed::new(FeedConfig::default(), &testing::scratch("redirect-policy")).unwrap();
        let start =
            Url::parse("https://api.github.com/repos/old-owner/ara/releases/assets/7").unwrap();
        let moved = feed
            .redirect(&start, "/repositories/123/releases/assets/7")
            .unwrap();
        let asset = feed
            .redirect(&moved, "https://release-assets.githubusercontent.com/asset")
            .unwrap();
        for url in [&start, &moved] {
            let header = authorization(url, Some("fixture-token")).unwrap().unwrap();
            assert_eq!(header, "Bearer fixture-token");
            assert!(header.is_sensitive());
        }
        assert!(
            authorization(&asset, Some("fixture-token"))
                .unwrap()
                .is_none()
        );
        let browser =
            Url::parse("https://github.com/old-owner/ara/releases/download/v1/file.zip").unwrap();
        let browser = feed
            .redirect(
                &browser,
                "https://github.com/Ara-Engine/ara/releases/download/v1/file.zip",
            )
            .unwrap();
        assert!(
            authorization(&browser, Some("fixture-token"))
                .unwrap()
                .is_none()
        );
        for target in [
            "http://api.github.com/repositories/123",
            "https://api.github.com:444/asset",
            "https://api.github.com.evil.test/asset",
            "https://user:secret@api.github.com/asset",
        ] {
            assert!(feed.redirect(&start, target).is_err());
        }
        assert!(feed.redirect(&asset, start.as_str()).is_err());
        assert!(authorization(&start, Some("invalid\nheader")).is_err());
        let legacy=crate::Asset{name:"engine.zip".into(),size:1,sha256:"00".repeat(32),url:"https://github.com/previous-owner/ara/releases/download/engine-v0.0.1-alpha/engine.zip".into()};
        assert_eq!(feed.asset_location(&legacy, true).unwrap(), legacy.url);
    }

    #[test]
    fn both_transports_follow_301_then_302_and_bound_redirect_loops() {
        let server = Server::new();
        for (path, status, target) in [
            ("/moved", 301, "/repositories/123/assets/7"),
            ("/repositories/123/assets/7", 302, "/asset"),
            ("/loop", 301, "/loop"),
        ] {
            server.set(
                path,
                Response {
                    status,
                    headers: vec![("Location".into(), target.into())],
                    ..Default::default()
                },
            );
        }
        server.set("/asset", Response::ok(b"verified fixture".to_vec()));
        let feed = Feed::new(server.config(), &testing::scratch("redirect-chain")).unwrap();
        let url = format!("{}/moved", server.base);
        assert_eq!(
            feed.request(&url, None, None).unwrap().text().unwrap(),
            "verified fixture"
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let bytes = runtime.block_on(async {
            feed.request_download(&url, None, &Cancel::default())
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap()
        });
        assert_eq!(&bytes[..], b"verified fixture");
        let requests = server.requests();
        assert_eq!(requests.len(), 6);
        assert!(
            requests
                .iter()
                .all(|request| !request.headers.contains_key("authorization"))
        );
        for chunk in requests.chunks(3) {
            assert_eq!(
                chunk.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
                ["/moved", "/repositories/123/assets/7", "/asset"]
            );
        }
        let loop_url = format!("{}/loop", server.base);
        assert_eq!(feed.request(&loop_url, None, None).unwrap_err().code, URL);
        let error = runtime
            .block_on(feed.request_download(&loop_url, None, &Cancel::default()))
            .unwrap_err();
        assert_eq!(error.code, URL);
        assert_eq!(
            server.requests().len() - requests.len(),
            2 * (MAX_REDIRECTS + 1)
        );
    }

    #[test]
    fn moved_api_pagination_stays_on_the_final_releases_endpoint() {
        let server = Server::new();
        server.set(
            "/repos/Ara-Engine/ara/releases?per_page=100",
            Response {
                status: 301,
                headers: vec![(
                    "Location".into(),
                    "/repositories/123/releases?per_page=100".into(),
                )],
                ..Default::default()
            },
        );
        server.set(
            "/repositories/123/releases?per_page=100",
            Response {
                headers: vec![(
                    "Link".into(),
                    format!(
                        "<{}/repositories/123/releases?per_page=100&page=2>; rel=\"next\"",
                        server.base
                    ),
                )],
                ..Response::ok(b"[]".to_vec())
            },
        );
        server.set(
            "/repositories/123/releases?per_page=100&page=2",
            Response::ok(b"[]".to_vec()),
        );
        let feed = Feed::new(server.config(), &testing::scratch("moved-pagination")).unwrap();
        assert!(feed.check().unwrap().releases.is_empty());
        assert_eq!(server.requests().len(), 3);
    }
}
