//! Authentication identities shared by the control plane and request path.
//!
//! Issuer selection deliberately uses an unverified JWT payload only as a
//! lookup key into a preconfigured set. No discovery request is made until
//! that exact issuer is already trusted; the selected validator then verifies
//! the signature and all registered claims before any identity is returned.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::auth::{OidcError, OidcValidator};
use crate::config::OidcConfig;
use crate::control_model::PrincipalIdentity;

#[derive(Debug, Clone, PartialEq)]
pub struct AuthenticatedSubject {
    pub principal: PrincipalIdentity,
    pub claims: HashMap<String, Value>,
}

/// A data-free authentication error. It can be logged or rendered in tests
/// without risking disclosure of the bearer token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityError {
    Malformed,
    UnknownIssuer,
    InvalidToken,
    MissingSubject,
}

pub struct TrustedIssuerSet {
    validators: HashMap<String, Arc<OidcValidator>>,
}

impl TrustedIssuerSet {
    pub fn new(configs: impl IntoIterator<Item = OidcConfig>) -> Self {
        let validators = configs
            .into_iter()
            .map(|config| {
                let issuer = config.issuer.clone();
                (issuer, Arc::new(OidcValidator::new(config)))
            })
            .collect();
        Self { validators }
    }

    /// Builds validators for browser ID tokens. The issuer and verification
    /// policy stay identical to API bearer validation, while the audience is
    /// explicitly the OIDC browser client's id rather than the API audience.
    pub fn new_for_browser(configs: impl IntoIterator<Item = OidcConfig>, client_id: &str) -> Self {
        Self::new(
            configs
                .into_iter()
                .map(|config| browser_oidc_config(config, client_id)),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.validators.is_empty()
    }

    pub async fn authenticate(&self, token: &str) -> Result<AuthenticatedSubject, IdentityError> {
        let unverified = jsonwebtoken::dangerous::insecure_decode::<Value>(token)
            .map_err(|_| IdentityError::Malformed)?;
        let issuer = unverified
            .claims
            .get("iss")
            .and_then(Value::as_str)
            .ok_or(IdentityError::UnknownIssuer)?;
        let validator = self
            .validators
            .get(issuer)
            .ok_or(IdentityError::UnknownIssuer)?;

        let claims_value = validator
            .decode_claims(token)
            .await
            .map_err(IdentityError::from)?;
        let subject = claims_value
            .get("sub")
            .and_then(Value::as_str)
            .filter(|subject| !subject.is_empty())
            .ok_or(IdentityError::MissingSubject)?;
        let claims = claims_value
            .as_object()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        Ok(AuthenticatedSubject {
            principal: PrincipalIdentity {
                issuer: issuer.to_string(),
                subject: subject.to_string(),
            },
            claims,
        })
    }

    /// Validates a browser-flow ID token and binds it to the login that
    /// initiated the flow. Only the stable principal leaves this boundary;
    /// the token's decoded claims remain internal to authentication.
    pub async fn authenticate_with_nonce(
        &self,
        token: &str,
        expected_nonce: &str,
    ) -> Result<PrincipalIdentity, IdentityError> {
        let subject = self.authenticate(token).await?;
        let nonce_matches = subject
            .claims
            .get("nonce")
            .and_then(Value::as_str)
            .is_some_and(|nonce| nonce == expected_nonce);
        if !nonce_matches {
            return Err(IdentityError::InvalidToken);
        }
        Ok(subject.principal)
    }
}

fn browser_oidc_config(mut config: OidcConfig, client_id: &str) -> OidcConfig {
    config.audience = client_id.to_string();
    config
}

impl From<OidcError> for IdentityError {
    fn from(_: OidcError) -> Self {
        Self::InvalidToken
    }
}

#[cfg(test)]
mod tests {
    use jsonwebtoken::jwk::Jwk;
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::RsaPrivateKey;
    use serde_json::json;

    use super::*;
    use crate::config::OidcClaimsConfig;

    #[tokio::test]
    async fn browser_id_token_requires_client_audience_and_exact_string_nonce() {
        let mut rng = rand::thread_rng();
        let private_key = RsaPrivateKey::new(&mut rng, 2_048).unwrap();
        let der = private_key.to_pkcs1_der().unwrap();
        let encoding_key = EncodingKey::from_rsa_der(der.as_bytes());
        let mut jwk = Jwk::from_encoding_key(&encoding_key, Algorithm::RS256).unwrap();
        jwk.common.key_id = Some("identity-kid".to_string());

        let issuer = "https://id.example.com".to_string();
        let config = OidcConfig {
            issuer: issuer.clone(),
            audience: "tellurion-api".to_string(),
            claims: OidcClaimsConfig {
                tenants: "tenants".to_string(),
                ..Default::default()
            },
            claims_authoritative: false,
            clock_skew_s: 5,
            jwks_ttl_s: 300,
        };
        let issuers = TrustedIssuerSet::new_for_browser([config], "control-ui");
        issuers
            .validators
            .get(&issuer)
            .unwrap()
            .seed_test_key("identity-kid", &jwk)
            .await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("identity-kid".to_string());
        let mint = |audience: &str, nonce: Option<serde_json::Value>| {
            let mut claims = json!({
                "iss": issuer,
                "aud": audience,
                "sub": "operator-1",
                "exp": now + 60
            });
            if let Some(nonce) = nonce {
                claims["nonce"] = nonce;
            }
            jsonwebtoken::encode(&header, &claims, &encoding_key).unwrap()
        };
        let browser_token = mint("control-ui", Some(json!("opaque-login-nonce")));

        let principal = issuers
            .authenticate_with_nonce(&browser_token, "opaque-login-nonce")
            .await
            .unwrap();
        assert_eq!(principal.subject, "operator-1");
        for (rejected, expected_nonce) in [
            (browser_token, "different-nonce"),
            (mint("control-ui", None), "opaque-login-nonce"),
            (mint("control-ui", Some(json!(7))), "opaque-login-nonce"),
            (
                mint("tellurion-api", Some(json!("opaque-login-nonce"))),
                "opaque-login-nonce",
            ),
        ] {
            assert_eq!(
                issuers
                    .authenticate_with_nonce(&rejected, expected_nonce)
                    .await,
                Err(IdentityError::InvalidToken)
            );
        }
    }
}
