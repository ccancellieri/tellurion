//! Compatibility transport for explicitly configured administrative sources.
//!
//! This type is deliberately separate from [`crate::PublicHttpsGateway`]. It
//! exists only so existing trusted storage configuration can continue to use
//! a remote COG while the public gateway remains HTTPS-only and brokered.

use std::ops::Range;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::{redirect::Policy, Client, StatusCode, Url};
use thiserror::Error;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct AdministrativeRangeObject {
    client: Client,
    url: Url,
    display_name: String,
}

#[derive(Debug, Error)]
pub enum AdministrativeSourceError {
    #[error("invalid administrative remote source")]
    Invalid,
    #[error("administrative remote source could not be read")]
    Transport,
    #[error("administrative remote source did not honor byte ranges")]
    Range,
}

impl AdministrativeRangeObject {
    /// Reads a trusted compatibility locator from one operator-configured
    /// environment variable. This feature is absent from the default crate
    /// API and does not accept request-supplied locators.
    pub fn from_env(variable: &str) -> Result<Self, AdministrativeSourceError> {
        let raw = std::env::var(variable).map_err(|_| AdministrativeSourceError::Invalid)?;
        Self::from_locator(&raw)
    }

    fn from_locator(raw: &str) -> Result<Self, AdministrativeSourceError> {
        let url = Url::parse(raw).map_err(|_| AdministrativeSourceError::Invalid)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(AdministrativeSourceError::Invalid);
        }
        let display_name = url
            .path_segments()
            .and_then(Iterator::last)
            .filter(|segment| !segment.is_empty())
            .map(|segment| match segment.rsplit_once('.') {
                Some((stem, _)) => stem.to_owned(),
                None => segment.to_owned(),
            })
            .unwrap_or_else(|| "cog".to_owned());
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| AdministrativeSourceError::Transport)?;
        Ok(Self {
            client,
            url,
            display_name,
        })
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub async fn get_range(
        &self,
        range: Range<u64>,
    ) -> Result<(u64, Bytes), AdministrativeSourceError> {
        if range.start >= range.end {
            return Err(AdministrativeSourceError::Range);
        }
        let requested = range.end - range.start;
        let mut response = self
            .client
            .get(self.url.clone())
            .header(RANGE, format!("bytes={}-{}", range.start, range.end - 1))
            .send()
            .await
            .map_err(|_| AdministrativeSourceError::Transport)?;
        if response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(AdministrativeSourceError::Range);
        }
        let total = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| parse_content_range(value, range.clone()))
            .ok_or(AdministrativeSourceError::Range)?;
        let mut body = BytesMut::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| AdministrativeSourceError::Transport)?
        {
            if body.len().saturating_add(chunk.len()) > requested as usize {
                return Err(AdministrativeSourceError::Range);
            }
            body.extend_from_slice(&chunk);
        }
        if body.len() as u64 != requested {
            return Err(AdministrativeSourceError::Range);
        }
        Ok((total, body.freeze()))
    }
}

fn parse_content_range(value: &str, requested: Range<u64>) -> Option<u64> {
    let value = value.strip_prefix("bytes ")?;
    let (interval, total) = value.split_once('/')?;
    let (start, end) = interval.split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?.checked_add(1)?;
    let total = total.parse::<u64>().ok()?;
    ((start..end) == requested && total >= end).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_source_reads_its_locator_from_the_named_environment_variable() {
        let variable = "TELLURION_HTTP_SOURCE_COMPAT_TEST";
        std::env::set_var(variable, "https://example.test/rasters/world.tif");
        let source = AdministrativeRangeObject::from_env(variable).unwrap();
        std::env::remove_var(variable);
        assert_eq!(source.display_name(), "world");
    }

    #[test]
    fn content_range_parser_accepts_only_the_exact_requested_interval() {
        assert_eq!(parse_content_range("bytes 4-7/10", 4..8), Some(10));
        assert_eq!(parse_content_range("bytes 4-7/*", 4..8), None);
        assert_eq!(parse_content_range("bytes 4-6/10", 4..8), None);
        assert_eq!(parse_content_range("nonsense", 4..8), None);
    }
}
