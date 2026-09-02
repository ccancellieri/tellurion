use std::sync::OnceLock;

use hmac::{Hmac, KeyInit, Mac};
use rand::Rng;
use sha2::Sha256;
use thiserror::Error;
use url::Url;

const MAX_URL_LENGTH: usize = 2_048;
type HmacSha256 = Hmac<Sha256>;

/// An eligible public locator. Its normalized form remains crate-private.
#[derive(Clone)]
pub struct PublicUrl {
    locator: Url,
    display_name: String,
    fingerprint: String,
}

impl PublicUrl {
    /// A hostname suitable for a user-facing label.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// A keyed, non-reversible identifier for diagnostics.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn locator(&self) -> &Url {
        &self.locator
    }

    pub(crate) fn host(&self) -> &str {
        &self.display_name
    }
}

impl std::fmt::Debug for PublicUrl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicUrl")
            .field("display_name", &self.display_name)
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

/// Stable URL rejection categories; raw locators are deliberately never kept.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Error)]
pub enum UrlValidationError {
    #[error("the locator is too long")]
    TooLong,
    #[error("the locator is not a valid absolute URL")]
    Invalid,
    #[error("only HTTPS locators are allowed")]
    Scheme,
    #[error("only port 443 is allowed")]
    Port,
    #[error("URL credentials are not allowed")]
    Credentials,
    #[error("URL query strings are not allowed")]
    Query,
    #[error("URL fragments are not allowed")]
    Fragment,
    #[error("the locator has an ambiguous encoded path")]
    EncodedPath,
    #[error("the locator has no hostname")]
    Host,
}

/// Validates and normalizes the narrow public URL policy.
pub fn validate_public_url(raw_url: &str) -> Result<PublicUrl, UrlValidationError> {
    if raw_url.len() > MAX_URL_LENGTH {
        return Err(UrlValidationError::TooLong);
    }

    let locator = Url::parse(raw_url).map_err(|_| UrlValidationError::Invalid)?;
    if locator.scheme() != "https" {
        return Err(UrlValidationError::Scheme);
    }
    if locator.port_or_known_default() != Some(443) {
        return Err(UrlValidationError::Port);
    }
    if contains_userinfo(raw_url) || !locator.username().is_empty() || locator.password().is_some()
    {
        return Err(UrlValidationError::Credentials);
    }
    if locator.query().is_some() {
        return Err(UrlValidationError::Query);
    }
    if locator.fragment().is_some() {
        return Err(UrlValidationError::Fragment);
    }
    if raw_path(raw_url).contains('%') {
        return Err(UrlValidationError::EncodedPath);
    }

    let display_name = locator
        .host_str()
        .ok_or(UrlValidationError::Host)?
        .to_owned();
    let fingerprint = keyed_fingerprint(locator.as_str());

    Ok(PublicUrl {
        locator,
        display_name,
        fingerprint,
    })
}

fn contains_userinfo(raw_url: &str) -> bool {
    let Some(after_scheme) = raw_url.split_once("://").map(|(_, after)| after) else {
        return false;
    };
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    authority.contains('@')
}

fn raw_path(raw_url: &str) -> &str {
    let Some(after_scheme) = raw_url.split_once("://").map(|(_, after)| after) else {
        return "";
    };
    after_scheme
        .find(['/', '?', '#'])
        .map(|start| &after_scheme[start..])
        .unwrap_or("")
}

pub(crate) fn keyed_fingerprint(value: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(process_secret()).expect("HMAC accepts fixed keys");
    mac.update(value.as_bytes());
    let bytes = mac.finalize().into_bytes();
    bytes[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn process_secret() -> &'static [u8; 32] {
    static SECRET: OnceLock<[u8; 32]> = OnceLock::new();
    SECRET.get_or_init(|| {
        let mut secret = [0_u8; 32];
        rand::rng().fill(&mut secret);
        secret
    })
}
