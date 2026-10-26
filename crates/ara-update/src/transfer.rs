//! Cancellable asset requests. Metadata uses the bounded blocking feed client.
use crate::{Cancel, Error, Feed, NETWORK, URL};
use reqwest::header;
use std::time::Duration;

impl Feed {
    pub(crate) async fn request_download(
        &self,
        text: &str,
        range: Option<u64>,
        cancel: &Cancel,
    ) -> Result<reqwest::Response, Error> {
        let builder = reqwest::Client::builder()
            .user_agent("Ara-Hub/1")
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(2 * 3600));
        #[cfg(any(test, feature = "test-support"))]
        let builder = if self.config.api.is_some() {
            builder.no_proxy()
        } else {
            builder
        };
        let client = builder
            .build()
            .map_err(|_| Error::new(NETWORK, "cannot initialize asset HTTPS client"))?;
        let mut url = self.url(text)?;
        for _ in 0..=crate::redirect::MAX_REDIRECTS {
            cancel.check()?;
            let mut request = client
                .get(url.clone())
                .header(header::ACCEPT, "application/octet-stream");
            if let Some(value) =
                crate::redirect::authorization(&url, std::env::var("GITHUB_TOKEN").ok().as_deref())?
            {
                request = request.header(header::AUTHORIZATION, value);
            }
            if let Some(offset) = range {
                request = request.header(header::RANGE, format!("bytes={offset}-"));
            }
            let response: Result<reqwest::Response, reqwest::Error> = tokio::select! {
                response = request.send() => response,
                error = cancel.cancelled() => return Err(error),
            };
            let response = response
                .map_err(|_| Error::new(NETWORK, "asset request interrupted; retry to resume"))?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|h| h.to_str().ok())
                    .ok_or_else(|| Error::new(URL, "asset redirect has no valid Location"))?;
                url = self.redirect(&url, location)?;
            } else {
                return Ok(response);
            }
        }
        Err(Error::new(URL, "too many asset redirects"))
    }
}
