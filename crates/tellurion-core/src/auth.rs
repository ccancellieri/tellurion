//! Tenant authentication/authorization (`#17`, OIDC half `#34`): the request
//! path calls `Resolver::resolve_tenant` to turn a URL's tenant external id
//! into the internal id everything below the HTTP boundary works in — this
//! is the seam a credentials -> tenant-authorization check sits in front of,
//! the one `router.rs`'s own `resolve_features` doc comment foreshadows
//! ("tenant/catalog stay explicit, required parameters here ... so a future
//! credentials -> tenant-claims check has one seam to sit in front of, not
//! several"). `TenantAuthorizer` is deliberately framework-free (no HTTP
//! types) so it lives here alongside the rest of the driver-agnostic
//! routing core, the same reasoning `problem.rs` documents for keeping RFC
//! 9457 bodies out of any one protocol crate. The server layer
//! (`tellurion-server`) extracts an `Authorization` header into a
//! [`Credential`], enforces the resulting [`AuthDecision`] as a 401/403
//! problem+json response, and never logs or echoes a credential's raw
//! value — see its own middleware doc.
//!
//! Absent `auth:` config (`AuthConfig::default()`, `is_configured() ==
//! false`) builds no authorizer at all: `ContextState::authorizer` is
//! `None`, and the server layer's enforcement middleware skips the
//! tenant-resolve + authorize call entirely rather than consulting an
//! "allow everything" implementation — byte-for-byte the pre-`#17`
//! behavior, not merely an equivalent one.
//!
//! Two credential sources feed the one [`StaticBearerAuthorizer`]
//! implementation, and both can be configured at once (`#34`):
//!
//! - A fixed bearer-token -> allowed-tenants map (`AuthConfig::bearer_tokens`,
//!   `#17`'s original slice) — dev/test tokens, or long-lived service
//!   credentials.
//! - OIDC/JWT bearer tokens (`AuthConfig::trusted_issuers`, plus the legacy
//!   singular `AuthConfig::oidc`, `#34`) — routed only to a preconfigured
//!   issuer, then verified against its published JWKS. See
//!   [`TrustedIssuerSet`] and [`OidcValidator`] for issuer selection,
//!   verification and JWKS caching.
//!
//! Where a static token's VALUE lives is a separate question from what it
//! authorizes, and since `#144` the document need not hold it: a
//! `bearer_tokens` entry names `token_env` (an environment variable) instead
//! of an inline `token`, resolved once per boot/reload by
//! [`resolve_bearer_credentials`]. That is the same seam
//! `StorageDecl::url_env` has always been for database credentials, one lane
//! over. Inline `token` still works, unchanged, and is reported by name —
//! see [`ResolvedBearerCredentials::inline_credential_warning`].
//!
//! [`StaticBearerAuthorizer::authorize`] tries the static map first: a
//! presented token that matches a configured static entry is authorized (or
//! rejected) from that map alone, before any JWT parsing is attempted — a
//! deliberately cheap, unambiguous first check, so a service account's
//! opaque static token is never run through JWT decoding at all. Only a
//! token that misses the static map falls through to OIDC validation, when
//! configured.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use tokio::sync::{Mutex, RwLock};

use crate::config::{oidc_endpoint_url_is_allowed, AuthConfig, OidcClaimsConfig, OidcConfig};
use crate::control_model::{ControlScope, PrincipalIdentity, RoleBinding};
use crate::identity::TrustedIssuerSet;

/// A request's presented credential, abstracted over any authentication
/// scheme so this trait never names an HTTP type. `Bearer` covers both the
/// static-token and the OIDC/JWT case — [`StaticBearerAuthorizer`] decides
/// which one a given value is by trying the static map first, then (if
/// configured) parsing it as a JWT.
pub enum Credential {
    /// No credential was presented at all — no `Authorization` header, or
    /// one in a scheme no authorizer here recognizes.
    None,
    /// The bearer token value from `Authorization: Bearer <token>`.
    Bearer(String),
}

// Manual `Debug`: never print the token value, even via a stray `{:?}` in a
// log line or a failed test assertion — see the module doc's "never logs or
// echoes" rule.
impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credential::None => f.write_str("Credential::None"),
            Credential::Bearer(_) => f.write_str("Credential::Bearer(<redacted>)"),
        }
    }
}

/// The result of a [`TenantAuthorizer::authorize`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthDecision {
    Allow,
    Deny(DenyReason),
}

/// Why a request was denied. The server layer maps this to a status code:
/// `NoCredential` -> 401, `NotAuthorized` -> 403. `NoCredential` covers both
/// "nothing was presented to authenticate at all" and, since `#34`, "a
/// bearer token was presented but is not a valid credential at all" (a
/// static-map miss with OIDC either unconfigured or unable to verify the
/// token as a signed-and-current JWT for this issuer/audience) — RFC 6750's
/// own `invalid_token` case is a 401, not a 403, for the same reason: 403
/// is reserved for a credential that *is* valid but doesn't cover the
/// target tenant. Deliberately carries no free-text detail — a `Deny`
/// reason must never be able to smuggle a credential's raw value into a log
/// line or a response body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    NoCredential,
    NotAuthorized,
}

/// A credential's full identity for the RBAC/ABAC policy layer (`#34`,
/// authorization directive 1: "membership is a first-class subject
/// attribute"): every tenant the credential holds membership in, the role
/// names it holds in each ([`memberships`](Self::memberships) — an empty
/// role set still means "a member," just with nothing an RBAC grant can
/// match, the same "member but no configured role" starting point every
/// subject has until an operator assigns one), plus whatever arbitrary
/// claims are available for ABAC filter-template substitution
/// ([`claims`](Self::claims), directive 5). [`anonymous`](Self::anonymous)
/// — no credential presented, or one that failed to establish any identity
/// at all — is the empty case: no memberships, no claims, matching this
/// type's own `Default`.
///
/// Deliberately separate from [`AuthDecision`]/[`DenyReason`]: those answer
/// one narrow question ("may this credential act as tenant X") the way
/// `#17` originally needed; `Subject` is the richer identity
/// `policy::authorize_resource` (`#34`) evaluates against a resource's
/// visibility and a tenant's role table. `TenantAuthorizer::authorize` and
/// `TenantAuthorizer::subject` are independent derivations from the same
/// credential — computing one never requires or implies the other, and
/// `authorize`'s own decision/status-code semantics are untouched by this
/// type's existence.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Subject {
    /// Tenant internal id -> role names held in that tenant.
    pub memberships: HashMap<String, HashSet<String>>,
    /// Arbitrary claims available for ABAC filter-template substitution
    /// (`{{claims.NAME}}`, see `config::GrantDecl::filter`'s own doc).
    pub claims: HashMap<String, serde_json::Value>,
    /// `#188`: a stable identity for this credential, used as the counter
    /// key of a [`RateScope::Principal`](crate::rate_limit::RateScope) rate
    /// condition — and used for nothing else. It takes no part in
    /// `authorize`, in platform isolation, or in RBAC/ABAC matching, all of
    /// which still key strictly off `memberships`/`claims` exactly as they
    /// did before this field existed.
    ///
    /// `None` for [`anonymous`](Self::anonymous). A static bearer token
    /// always resolves to `Some`: its declared
    /// [`BearerTokenDecl::principal`](crate::config::BearerTokenDecl::principal)
    /// when it has one, else the same short, non-reversible token
    /// fingerprint (`token:<12 hex>`) the platform-admin audit trail
    /// already falls back to (`#110`) — so a ceiling can bound a token the
    /// operator never bothered to name. Without that fallback every unnamed
    /// token would share one bucket, which throttles them collectively for
    /// no reason anyone declared. An OIDC token resolves to the exact
    /// `issuer#sub` pair, keeping subjects from different identity providers
    /// in separate counter namespaces. A token without a non-empty string
    /// `sub` is rejected before a subject is created.
    pub principal: Option<String>,
    /// Verified issuer-qualified identity. Raw token claims and static-token
    /// fingerprints never populate this field.
    pub identity: Option<PrincipalIdentity>,
}

impl Subject {
    /// No credential presented, or one that failed to establish any
    /// identity — empty memberships, empty claims. Matches this type's own
    /// `Default`; a named constructor reads more clearly at call sites than
    /// `Subject::default()` does for what is, semantically, "the anonymous
    /// subject," not merely "a subject with nothing filled in yet."
    pub fn anonymous() -> Self {
        Self::default()
    }

    /// Whether this subject holds membership (any role set, including
    /// empty) in `tenant_id` — the platform isolation check's membership
    /// half (authorization directive 2).
    pub fn is_member_of(&self, tenant_id: &str) -> bool {
        self.memberships.contains_key(tenant_id)
    }
}

/// The tenant trust-boundary seam (`#17`): given a request's `credential`
/// and the target tenant's internal id, decide whether the request may act
/// as that tenant. Implementations must never let a credential's raw value
/// reach a log line, an error, or a returned [`DenyReason`].
#[async_trait::async_trait]
pub trait TenantAuthorizer: Send + Sync {
    async fn authorize(&self, credential: &Credential, tenant_id: &str) -> AuthDecision;

    /// Derives the credential's full [`Subject`] (`#34`) — every tenant
    /// membership, the roles held in each, and whatever claims are
    /// available for ABAC substitution. Independent of, and never consulted
    /// by, [`authorize`](Self::authorize): the policy layer
    /// (`policy::authorize_resource`) is this method's only caller. A
    /// credential that fails to establish any identity at all (an invalid
    /// or expired token, same as a missing one) resolves to
    /// [`Subject::anonymous`], never an error — mirroring `authorize`'s own
    /// "an invalid credential conveys no more than no credential at all"
    /// treatment, just without a distinguishing status code to preserve
    /// here (the policy layer's own `Credential::None` vs `Credential::
    /// Bearer` check at its call site is what recovers 401-vs-403, when it
    /// needs to).
    async fn subject(&self, credential: &Credential) -> Subject;

    /// `#110`: whether `credential` carries platform-level administrative
    /// authority — the config-mutation control lane's own gate
    /// (`tellurion-server::config_mutation`), orthogonal to any tenant.
    /// `authorize`'s question is "may this act as tenant X"; this one is
    /// "may this mutate the platform's own configuration document at all."
    /// See [`PlatformAdminDecision`]'s own doc for the principal it carries
    /// on `Allow`.
    async fn authorize_platform_admin(&self, credential: &Credential) -> PlatformAdminDecision;
}

/// The outcome of a platform-admin authorization check (`#110`). `Allow`
/// carries a human-identifiable `principal` for the config-mutation audit
/// trail (`tellurion_core::audit`) — never the raw credential value; see
/// this module's own "never logs or echoes" rule. A token that authorizes
/// but declares no `config::BearerTokenDecl::principal` of its own falls
/// back to a short, non-reversible fingerprint of the token (SHA-256
/// truncated to 12 hex characters), so an audit entry always names SOME
/// identifier without ever being able to leak the token itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformAdminDecision {
    Allow { principal: String },
    Deny(DenyReason),
}

/// `#17`'s (and `#34`'s) concrete authorizer: a fixed map from bearer token
/// value to the set of tenant *internal* ids that token authorizes — the
/// same internal-id convention `config::CatalogDecl::tenant` uses to
/// reference a tenant from elsewhere in the config document — plus an
/// optional [`OidcValidator`] consulted for any bearer token the static map
/// doesn't recognize. Built once from [`AuthConfig`] at `Router::build` time
/// (or reload, via [`build_authorizer`]) — the static map costs no I/O to
/// build; the OIDC validator's JWKS cache starts cold and is filled lazily
/// by the first request that needs it (see [`OidcValidator`]'s own doc), so
/// building this is never blocked on the identity provider either.
/// One static token's full authorization/subject data. `tenants` feeds
/// `authorize` (`#17`, unchanged); `roles`/`claims` (`#34`) additionally
/// feed `subject` — see `Subject`'s own doc. Every `roles` key is a subset
/// of `tenants` by construction (`AppConfig::validate` enforces this on the
/// `config::BearerTokenDecl` this is built from).
struct TokenEntry {
    tenants: HashSet<String>,
    roles: HashMap<String, HashSet<String>>,
    claims: HashMap<String, serde_json::Value>,
    /// `#110`: platform-admin authority, and the audit-trail principal to
    /// report on `Allow` — see `config::BearerTokenDecl::platform_admin`/
    /// `::principal`'s own docs.
    platform_admin: bool,
    principal: Option<String>,
}

pub struct StaticBearerAuthorizer {
    /// token value -> this token's full entry.
    tokens: HashMap<String, TokenEntry>,
    oidc: Option<Arc<OidcValidator>>,
    trusted_issuers: Option<Arc<TrustedIssuerSet>>,
    oidc_claim_mappings: HashMap<String, OidcClaimsConfig>,
    platform_admins: HashSet<PrincipalIdentity>,
    tenant_bindings: HashMap<PrincipalIdentity, HashMap<String, HashSet<String>>>,
}

impl StaticBearerAuthorizer {
    pub fn new(entries: impl IntoIterator<Item = (String, Vec<String>)>) -> Self {
        Self::with_oidc(entries, None)
    }

    /// Builds from the plain `(token, tenants)` shape every existing caller
    /// (this module's own tests, and `#17`'s original slice) already uses —
    /// each entry gets empty `roles`/`claims`, the same "member, but no
    /// role a policy grant can match" starting point `Subject`'s own doc
    /// describes.
    fn with_oidc(
        entries: impl IntoIterator<Item = (String, Vec<String>)>,
        oidc: Option<Arc<OidcValidator>>,
    ) -> Self {
        let tokens = entries
            .into_iter()
            .map(|(token, tenants)| {
                (
                    token,
                    TokenEntry {
                        tenants: tenants.into_iter().collect(),
                        roles: HashMap::new(),
                        claims: HashMap::new(),
                        platform_admin: false,
                        principal: None,
                    },
                )
            })
            .collect();
        Self {
            tokens,
            oidc,
            trusted_issuers: None,
            oidc_claim_mappings: HashMap::new(),
            platform_admins: HashSet::new(),
            tenant_bindings: HashMap::new(),
        }
    }

    fn with_trusted_issuers(
        entries: impl IntoIterator<Item = (String, TokenEntry)>,
        trusted_issuers: Option<Arc<TrustedIssuerSet>>,
        oidc_claim_mappings: HashMap<String, OidcClaimsConfig>,
        platform_admins: HashSet<PrincipalIdentity>,
        tenant_bindings: HashMap<PrincipalIdentity, HashMap<String, HashSet<String>>>,
    ) -> Self {
        Self {
            tokens: entries.into_iter().collect(),
            oidc: None,
            trusted_issuers,
            oidc_claim_mappings,
            platform_admins,
            tenant_bindings,
        }
    }

    async fn authenticated_oidc_subject(&self, token: &str) -> Result<Subject, ()> {
        let trusted = self.trusted_issuers.as_ref().ok_or(())?;
        let authenticated = trusted.authenticate(token).await.map_err(|_| ())?;
        let claims_value =
            serde_json::Value::Object(authenticated.claims.clone().into_iter().collect());
        let mut memberships: HashMap<String, HashSet<String>> = HashMap::new();
        if let Some(mapping) = self
            .oidc_claim_mappings
            .get(&authenticated.principal.issuer)
        {
            let tenants = string_set_from_claim(&claims_value, &mapping.tenants);
            let roles = mapping
                .roles
                .as_ref()
                .map(|claim| string_set_from_claim(&claims_value, claim))
                .unwrap_or_default();
            memberships.extend(tenants.into_iter().map(|tenant| (tenant, roles.clone())));
        }
        if let Some(bindings) = self.tenant_bindings.get(&authenticated.principal) {
            for (tenant_id, bound_roles) in bindings {
                memberships
                    .entry(tenant_id.clone())
                    .or_default()
                    .extend(bound_roles.iter().cloned());
            }
        }
        let principal = format!(
            "{}#{}",
            authenticated.principal.issuer, authenticated.principal.subject
        );
        Ok(Subject {
            memberships,
            claims: authenticated.claims,
            principal: Some(principal),
            identity: Some(authenticated.principal),
        })
    }
}

#[async_trait::async_trait]
impl TenantAuthorizer for StaticBearerAuthorizer {
    async fn authorize(&self, credential: &Credential, tenant_id: &str) -> AuthDecision {
        let token = match credential {
            Credential::None => return AuthDecision::Deny(DenyReason::NoCredential),
            Credential::Bearer(token) => token,
        };

        // Static map wins first, cheap and unambiguous: no JWT parsing is
        // ever attempted for a token that's already a known static entry —
        // see the module doc.
        if let Some(entry) = self.tokens.get(token) {
            return if entry.tenants.contains(tenant_id) {
                AuthDecision::Allow
            } else {
                AuthDecision::Deny(DenyReason::NotAuthorized)
            };
        }

        if self.trusted_issuers.is_some() {
            return match self.authenticated_oidc_subject(token).await {
                Ok(subject) if subject.is_member_of(tenant_id) => AuthDecision::Allow,
                Ok(_) => AuthDecision::Deny(DenyReason::NotAuthorized),
                Err(()) => AuthDecision::Deny(DenyReason::NoCredential),
            };
        }

        let Some(oidc) = &self.oidc else {
            return AuthDecision::Deny(DenyReason::NotAuthorized);
        };

        match oidc.validate(token).await {
            Ok(memberships) if memberships.contains(tenant_id) => AuthDecision::Allow,
            Ok(_) => AuthDecision::Deny(DenyReason::NotAuthorized),
            // Bad signature, expired, wrong iss/aud, unknown kid, malformed
            // token, ... — none of these get to see a `NotAuthorized`
            // (403): the token was never established as a valid credential
            // in the first place, so this is a 401. `oidc.validate`'s own
            // error type carries no detail derived from the token's raw
            // value — see its own doc.
            Err(_) => AuthDecision::Deny(DenyReason::NoCredential),
        }
    }

    /// `#34`: same static-map-first, OIDC-on-miss decision order as
    /// `authorize`, but resolving to a full [`Subject`] rather than a single
    /// tenant's allow/deny. A static match never touches the JWKS endpoint,
    /// same as `authorize`. An OIDC token that fails verification resolves
    /// to [`Subject::anonymous`] — see this method's own trait doc.
    async fn subject(&self, credential: &Credential) -> Subject {
        let token = match credential {
            Credential::None => return Subject::anonymous(),
            Credential::Bearer(token) => token,
        };

        if let Some(entry) = self.tokens.get(token) {
            let memberships = entry
                .tenants
                .iter()
                .map(|tenant| {
                    let roles = entry.roles.get(tenant).cloned().unwrap_or_default();
                    (tenant.clone(), roles)
                })
                .collect();
            return Subject {
                memberships,
                claims: entry.claims.clone(),
                // `#188`: same declared-principal-then-fingerprint fallback
                // `authorize_platform_admin` uses — see `Subject::principal`.
                principal: Some(
                    entry
                        .principal
                        .clone()
                        .unwrap_or_else(|| token_fingerprint(token)),
                ),
                identity: entry.principal.as_ref().map(|principal| PrincipalIdentity {
                    issuer: "urn:tellurion:static".to_string(),
                    subject: principal.clone(),
                }),
            };
        }

        if self.trusted_issuers.is_some() {
            return self
                .authenticated_oidc_subject(token)
                .await
                .unwrap_or_else(|_| Subject::anonymous());
        }

        let Some(oidc) = &self.oidc else {
            return Subject::anonymous();
        };
        oidc.subject(token)
            .await
            .unwrap_or_else(|_| Subject::anonymous())
    }

    /// `#110`: same static-map-first, OIDC-on-miss decision order as
    /// `authorize`/`subject`. OIDC has no platform-admin claim modeled yet
    /// — a token that misses the static map is, at most, a validly
    /// authenticated TENANT credential, never a platform admin; it still
    /// distinguishes 401 (never established as a credential at all) from
    /// 403 (a real credential, just not this one), same as `authorize`.
    async fn authorize_platform_admin(&self, credential: &Credential) -> PlatformAdminDecision {
        let token = match credential {
            Credential::None => return PlatformAdminDecision::Deny(DenyReason::NoCredential),
            Credential::Bearer(token) => token,
        };

        if let Some(entry) = self.tokens.get(token) {
            let bound_static_admin = entry.principal.as_ref().is_some_and(|subject| {
                self.platform_admins.contains(&PrincipalIdentity {
                    issuer: "urn:tellurion:static".to_string(),
                    subject: subject.clone(),
                })
            });
            return if entry.platform_admin || bound_static_admin {
                let principal = entry
                    .principal
                    .clone()
                    .unwrap_or_else(|| token_fingerprint(token));
                PlatformAdminDecision::Allow { principal }
            } else {
                PlatformAdminDecision::Deny(DenyReason::NotAuthorized)
            };
        }

        if let Some(trusted) = &self.trusted_issuers {
            return match trusted.authenticate(token).await {
                Ok(authenticated) if self.platform_admins.contains(&authenticated.principal) => {
                    PlatformAdminDecision::Allow {
                        principal: format!(
                            "{}#{}",
                            authenticated.principal.issuer, authenticated.principal.subject
                        ),
                    }
                }
                Ok(_) => PlatformAdminDecision::Deny(DenyReason::NotAuthorized),
                Err(_) => PlatformAdminDecision::Deny(DenyReason::NoCredential),
            };
        }

        let Some(oidc) = &self.oidc else {
            return PlatformAdminDecision::Deny(DenyReason::NotAuthorized);
        };
        match oidc.validate(token).await {
            // `#110`: OIDC never grants platform-admin authority yet — a
            // token that verifies fine as a tenant credential still isn't a
            // platform admin.
            Ok(_) => PlatformAdminDecision::Deny(DenyReason::NotAuthorized),
            Err(_) => PlatformAdminDecision::Deny(DenyReason::NoCredential),
        }
    }
}

/// A short, non-reversible name for a token value — the audit trail's
/// principal fallback (`#110`) and, since `#144`, how an inline-credential
/// report names a principal that declared no `principal:` of its own. Twelve
/// hex characters of SHA-256: enough to tell two principals apart in a log,
/// never enough to present as a credential.
fn token_fingerprint(token: &str) -> String {
    format!(
        "token:{}",
        &crate::sigv4::sha256_hex(token.as_bytes())[..12]
    )
}

/// Every static bearer principal with its token value resolved from wherever
/// this deployment keeps it (`#144`).
///
/// The point of the type is that the values are *behind* it: no `Serialize`,
/// no public field, and a [`Debug`] that counts principals instead of
/// printing them. A resolved credential must be able to reach
/// [`StaticBearerAuthorizer`] and nowhere else — in particular it is never
/// written back into an [`AuthConfig`], which would put it straight into the
/// control-store snapshot and the `GET /config` response body.
///
/// Deliberately not `Clone` either: it is built once, at boot or reload, and
/// moved into the authorizer. Nothing needs a second copy of a secret.
pub struct ResolvedBearerCredentials {
    entries: Vec<(String, TokenEntry)>,
    inline_principals: Vec<String>,
}

// Manual `Debug` for the same reason [`Credential`] has one: a stray `{:?}`
// in a log line or a failed assertion must not be able to print a token.
impl std::fmt::Debug for ResolvedBearerCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedBearerCredentials")
            .field("principals", &self.entries.len())
            .field("inline_principals", &self.inline_principals)
            .finish()
    }
}

impl ResolvedBearerCredentials {
    /// The principals whose token value is written inline in the
    /// configuration document, named by their declared `principal:` or, for
    /// one that declares none, by [`token_fingerprint`] — never by value.
    /// Empty when every principal reads its token from the environment,
    /// which is also the empty case for a deployment with no static tokens
    /// at all.
    pub fn inline_principals(&self) -> &[String] {
        &self.inline_principals
    }

    /// The one line a deployment still carrying inline credentials must see,
    /// or `None` when there is nothing to say.
    ///
    /// A named, loud deprecation that still boots — deliberately not a
    /// refusal. Every config written before `#144` declares its tokens
    /// inline, and refusing them at boot would take those deployments down
    /// to fix a defect they cannot fix mid-restart; the same reasoning makes
    /// `ControlStoreLocator::LegacyFile` the `Default` rather than making
    /// the block required. Silence is the other thing it must not be: the
    /// credentials really are readable by anyone who can read the document
    /// or a control-store dump, and an operator who is never told will never
    /// move them.
    ///
    /// Returned as a value rather than only logged so a test can assert on
    /// it directly, the way the boot path does.
    pub fn inline_credential_warning(&self) -> Option<String> {
        if self.inline_principals.is_empty() {
            return None;
        }
        Some(format!(
            "auth.bearer_tokens: {} principal(s) carry an inline 'token' value in the configuration document, \
             readable by anyone who can read that document or a control-store snapshot of it; \
             declare 'token_env' naming an environment variable instead (#144). Principals: {}",
            self.inline_principals.len(),
            self.inline_principals.join(", ")
        ))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Resolves every `auth.bearer_tokens` entry against the process environment
/// (`#144`) — the credential-storage seam.
///
/// `lookup` is the seam itself: the process environment here, and the one
/// parameter a future credential store (a relational one, per the issue)
/// substitutes without any of the shape above moving. Kept a plain function
/// parameter rather than a trait until a second implementation actually
/// exists, so tests read the environment they declare and never mutate the
/// process's.
///
/// Every failure is named and refuses:
///
/// - a `token_env` naming a variable that is not set, or set to the empty
///   string — never a principal that silently stops authorizing, which is
///   the failure mode an operator diagnoses as "the credential was revoked";
/// - two entries resolving to the same token value, the same collision
///   `AppConfig::validate` already refuses for inline values (the authorizer
///   is a map keyed by token, so the second entry could never be reached).
///
/// No error message ever carries a token value: the variable NAME is not a
/// secret, and it is the only thing an operator needs to fix the problem.
pub fn resolve_bearer_credentials(auth: &AuthConfig) -> crate::Result<ResolvedBearerCredentials> {
    resolve_bearer_credentials_from(auth, |name| std::env::var(name).ok())
}

/// [`resolve_bearer_credentials`] against an explicit lookup — see its doc.
pub fn resolve_bearer_credentials_from(
    auth: &AuthConfig,
    lookup: impl Fn(&str) -> Option<String>,
) -> crate::Result<ResolvedBearerCredentials> {
    let mut entries: Vec<(String, TokenEntry)> = Vec::with_capacity(auth.bearer_tokens.len());
    let mut inline_principals = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for decl in &auth.bearer_tokens {
        let token = match decl.token_env.as_deref() {
            Some(name) => {
                let value = lookup(name).ok_or_else(|| {
                    crate::Error::Config(format!(
                        "auth.bearer_tokens: token_env names '{name}', but that environment variable is not set"
                    ))
                })?;
                if value.is_empty() {
                    return Err(crate::Error::Config(format!(
                        "auth.bearer_tokens: token_env names '{name}', but that environment variable is empty"
                    )));
                }
                value
            }
            None => {
                inline_principals.push(
                    decl.principal
                        .clone()
                        .unwrap_or_else(|| token_fingerprint(&decl.token)),
                );
                decl.token.clone()
            }
        };
        if !seen.insert(token.clone()) {
            return Err(crate::Error::Config(match decl.token_env.as_deref() {
                Some(name) => format!(
                    "auth.bearer_tokens: token_env '{name}' resolves to the same token value as an earlier entry"
                ),
                None => "auth.bearer_tokens: duplicate token entry".to_string(),
            }));
        }
        entries.push((
            token,
            TokenEntry {
                tenants: decl.tenants.iter().cloned().collect(),
                roles: decl
                    .roles
                    .iter()
                    .map(|(tenant, roles)| (tenant.clone(), roles.iter().cloned().collect()))
                    .collect(),
                claims: decl.claims.clone(),
                platform_admin: decl.platform_admin,
                principal: decl.principal.clone(),
            },
        ));
    }
    Ok(ResolvedBearerCredentials {
        entries,
        inline_principals,
    })
}

/// Builds the authorizer `auth` selects, alongside `Router::build` (`#17`,
/// `#34`): `None` when `auth.is_configured()` is `false` (see the module
/// doc's "byte-for-byte" rule — the server layer must skip enforcement
/// entirely in this case, not receive a permissive authorizer to consult),
/// else a [`StaticBearerAuthorizer`] wrapping `auth.bearer_tokens` and,
/// when `auth.oidc` is set, a freshly constructed [`OidcValidator`] for it.
/// Building the validator here does no network I/O (see its own doc) — this
/// function stays exactly as safe to call from a config *reload* as it
/// always was, including under an issuer that's unreachable or doesn't
/// exist: the reload still validate-then-swaps in the new state, and JWKS
/// discovery is deferred to the first token that actually needs it. The
/// caller (`main.rs` at boot, `reload.rs` on every reload attempt) threads
/// the result into `AppContext::new`/`AppContext::reload` alongside the
/// `Router` and `Resolver` built from the same config, so all three stay
/// one atomically-swapped unit.
///
/// Fallible since `#144`: resolving a `token_env` that names an unset
/// variable is a named `Err`, which fails boot the way a missing
/// `storages[].url_env` already does and, on a reload, leaves the previous
/// configuration serving rather than swapping in an authorizer missing a
/// principal. `Ok(None)` remains the permissive unconfigured case and is not
/// a failure at all.
pub fn build_authorizer(auth: &AuthConfig) -> crate::Result<Option<Arc<dyn TenantAuthorizer>>> {
    build_authorizer_with_bindings(auth, &[])
}

/// Builds an authorizer from one durable control snapshot. Platform
/// administration is granted only by an exact `(issuer, sub)` `sysadmin`
/// binding at platform scope; token role claims are never consulted for it.
pub fn build_authorizer_with_bindings(
    auth: &AuthConfig,
    role_bindings: &[RoleBinding],
) -> crate::Result<Option<Arc<dyn TenantAuthorizer>>> {
    if !auth.is_configured() {
        return Ok(None);
    }
    let issuer_configs: Vec<_> = auth
        .oidc
        .iter()
        .chain(auth.trusted_issuers.iter())
        .cloned()
        .collect();
    let oidc_claim_mappings = auth
        .oidc
        .iter()
        .map(|config| (config.issuer.clone(), config.claims.clone()))
        .chain(
            auth.trusted_issuers
                .iter()
                .filter(|config| config.claims_authoritative)
                .map(|config| (config.issuer.clone(), config.claims.clone())),
        )
        .collect();
    let trusted_issuers = if issuer_configs.is_empty() {
        None
    } else {
        Some(Arc::new(TrustedIssuerSet::new(issuer_configs)))
    };
    let platform_admins = role_bindings
        .iter()
        .filter(|binding| binding.role == "sysadmin" && binding.scope == ControlScope::Platform)
        .map(|binding| binding.principal.clone())
        .collect();
    let mut tenant_bindings: HashMap<PrincipalIdentity, HashMap<String, HashSet<String>>> =
        HashMap::new();
    for binding in role_bindings {
        if let ControlScope::Tenant { tenant_id } = &binding.scope {
            tenant_bindings
                .entry(binding.principal.clone())
                .or_default()
                .entry(tenant_id.clone())
                .or_default()
                .insert(binding.role.clone());
        }
    }
    // `#144`: the one place a static token's value is read. Fails by name
    // before anything is built, so a half-resolved authorizer never exists.
    let credentials = resolve_bearer_credentials(auth)?;
    // Once per boot and once per reload, deliberately: a warning logged only
    // at first boot is one an operator who inherited the deployment never
    // sees. The line names principals, never values.
    if let Some(warning) = credentials.inline_credential_warning() {
        tracing::warn!("{warning}");
    }
    Ok(Some(Arc::new(StaticBearerAuthorizer::with_trusted_issuers(
        credentials.entries,
        trusted_issuers,
        oidc_claim_mappings,
        platform_admins,
        tenant_bindings,
    )) as Arc<dyn TenantAuthorizer>))
}

/// Why [`OidcValidator::validate`] rejected a token. Crate-private and
/// carries no data derived from the token's raw value on purpose —
/// [`StaticBearerAuthorizer::authorize`] collapses every variant to the
/// same [`DenyReason::NoCredential`], so this type exists only to let
/// `validate`'s implementation be readable; it is never formatted into a
/// log line or response body anywhere on the request path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OidcError {
    /// Not a well-formed `header.payload.signature` JWT.
    Malformed,
    /// The header's `alg` isn't one this validator accepts (RS256/ES256
    /// only — see the module doc). Rejected before any JWKS lookup, the
    /// same defense-in-depth `jsonwebtoken` itself also applies (a
    /// decoding key's algorithm family must match the token header's) but
    /// worth failing fast on here too: an attacker-chosen `alg` (the
    /// classic RS256->HS256 downgrade, or `none`) never reaches key
    /// resolution at all.
    UnsupportedAlgorithm,
    /// No `kid` in the header — this validator only supports JWKS-based
    /// verification, which requires one to select a key.
    MissingKid,
    /// The `kid` doesn't match any key in the current (possibly
    /// just-refreshed) JWKS — see [`JwksCache`]'s own doc for the bounded
    /// refresh behavior this implies.
    UnknownKid,
    /// The JWKS's declared algorithm for this `kid` doesn't match the
    /// token header's `alg`.
    AlgorithmMismatch,
    /// Signature verification or `iss`/`aud`/`exp`/`nbf` validation failed.
    InvalidToken,
}

/// OIDC bearer-token verification (`#34`): validates a JWT's signature
/// against `issuer`'s published JWKS, then `iss`/`aud`/`exp`/`nbf` with
/// `clock_skew` leeway, and returns the tenant memberships read from
/// `tenant_claim` on success. Only RS256 and ES256 (P-256) are supported —
/// whatever a JWK's `kty` (and, for EC, `crv`) implies; a JWKS entry of any
/// other key type is simply never selected, not treated as an error (an
/// IdP publishing an EdDSA key alongside an RS256 one shouldn't break the
/// RS256 one). Holds the [`JwksCache`] (its own TTL/single-flight design is
/// documented on that type) and an HTTP client (`reqwest`, 5s per-request
/// timeout so a slow/unreachable issuer can never hang a request-path
/// caller indefinitely — see [`JwksCache::resolve`]'s single-flight gate,
/// which a hung fetch would otherwise block every other in-flight refresh
/// attempt behind).
///
/// One `OidcValidator` is built per `auth.oidc` config section (never
/// per-request) and lives inside the same reload-swapped
/// `Arc<dyn TenantAuthorizer>` as the rest of the authorizer (see
/// `context.rs`) — a config reload builds a brand new validator with a
/// cold JWKS cache, never mutates one in place, so an in-flight request
/// always sees a JWKS cache state that belongs to a single, consistent
/// config snapshot.
pub struct OidcValidator {
    issuer: String,
    audience: String,
    tenant_claim: String,
    /// `#34`: the claim role names are read from, when configured. `None`
    /// means an OIDC-authenticated subject always resolves to an empty role
    /// set — see `config::OidcClaimsConfig::roles`'s own doc.
    roles_claim: Option<String>,
    clock_skew_s: u64,
    http: reqwest::Client,
    jwks: JwksCache,
}

impl OidcValidator {
    /// Builds a validator from config. Does no I/O: the JWKS cache starts
    /// empty and the first call to [`validate`](Self::validate) that needs
    /// a key triggers the first fetch — see [`JwksCache`]'s own doc. This
    /// is the load-bearing property that keeps a config reload from ever
    /// blocking on the identity provider (`AppConfig::validate` similarly
    /// does no network probe of `issuer` — see that method's own doc).
    pub fn new(config: OidcConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .redirect(oidc_redirect_policy())
            .build()
            .unwrap_or_default();
        let ttl = Duration::from_secs(config.jwks_ttl_s);
        Self {
            issuer: config.issuer,
            audience: config.audience,
            tenant_claim: config.claims.tenants,
            roles_claim: config.claims.roles,
            clock_skew_s: config.clock_skew_s,
            http,
            jwks: JwksCache::new(ttl),
        }
    }

    #[cfg(test)]
    pub(crate) async fn seed_test_key(&self, kid: &str, jwk: &Jwk) {
        let algorithm = algorithm_for_jwk(jwk).expect("test JWK must use a supported algorithm");
        let decoding_key = DecodingKey::from_jwk(jwk).expect("test JWK must be decodable");
        let mut state = self.jwks.state.write().await;
        state.keys.insert(
            kid.to_string(),
            CachedKey {
                decoding_key,
                algorithm,
            },
        );
        state.last_attempt_at = Some(Instant::now());
    }

    /// Verifies `token` as a JWT issued by `self.issuer` for
    /// `self.audience` (signature, `iss`/`aud`/`exp`/`nbf`), and returns its
    /// full decoded claims on success. Shared by [`validate`](Self::validate)
    /// (`#17`/`#34`'s original tenant-only extraction) and
    /// [`subject`](Self::subject) (`#34`'s fuller `Subject` derivation) so
    /// both go through exactly one signature-verification path. Never
    /// includes `token` (or any substring of it) in the returned error —
    /// see [`OidcError`]'s own doc.
    pub(crate) async fn decode_claims(&self, token: &str) -> Result<serde_json::Value, OidcError> {
        let header = jsonwebtoken::decode_header(token).map_err(|_| OidcError::Malformed)?;
        if header.alg != Algorithm::RS256 && header.alg != Algorithm::ES256 {
            return Err(OidcError::UnsupportedAlgorithm);
        }
        let kid = header.kid.as_deref().ok_or(OidcError::MissingKid)?;
        let key = self.jwks.resolve(&self.http, &self.issuer, kid).await?;
        if key.algorithm != header.alg {
            return Err(OidcError::AlgorithmMismatch);
        }

        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.audience.as_str()]);
        validation.leeway = self.clock_skew_s;
        validation.validate_nbf = true;
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);

        let data = jsonwebtoken::decode::<serde_json::Value>(token, &key.decoding_key, &validation)
            .map_err(|_| OidcError::InvalidToken)?;
        Ok(data.claims)
    }

    /// Verifies `token` and returns the tenant memberships read from
    /// `self.tenant_claim` on success — `#17`/`#34`'s original extraction,
    /// unchanged. See [`subject`](Self::subject) for `#34`'s fuller
    /// derivation.
    async fn validate(&self, token: &str) -> Result<HashSet<String>, OidcError> {
        let claims = self.decode_claims(token).await?;
        Ok(string_set_from_claim(&claims, &self.tenant_claim))
    }

    /// `#34`: verifies `token` and returns its full [`Subject`] — tenant
    /// memberships (`self.tenant_claim`), the role set read from
    /// `self.roles_claim` when configured (applied uniformly across every
    /// membership — see `config::OidcClaimsConfig::roles`'s own doc for why
    /// this is flat rather than per-tenant), and the token's complete raw
    /// claims object for ABAC substitution. A token whose claims object
    /// isn't a JSON object at all (never happens for a real JWT, whose
    /// payload is always an object, but `serde_json::Value` doesn't rule it
    /// out at the type level) degrades to an empty claims map rather than
    /// panicking.
    async fn subject(&self, token: &str) -> Result<Subject, OidcError> {
        let claims_value = self.decode_claims(token).await?;
        let tenants = string_set_from_claim(&claims_value, &self.tenant_claim);
        let roles = match &self.roles_claim {
            Some(claim_name) => string_set_from_claim(&claims_value, claim_name),
            None => HashSet::new(),
        };
        let memberships = tenants
            .into_iter()
            .map(|tenant| (tenant, roles.clone()))
            .collect();
        let claims = claims_value
            .as_object()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        // `#188`: `sub` is the standard subject identifier; a token without
        // a string one carries no principal at all rather than a made-up
        // one — see `Subject::principal`.
        let principal = claims_value
            .get("sub")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        Ok(Subject {
            memberships,
            claims,
            principal: principal.clone(),
            identity: principal.map(|subject| PrincipalIdentity {
                issuer: self.issuer.clone(),
                subject,
            }),
        })
    }
}

/// Reads a set of strings from `claims[claim_name]`, accepting either a JSON
/// array of strings or a single space-separated string (the OAuth2 `scope`
/// convention — see `OidcClaimsConfig::tenants`'s own doc). Any other shape
/// (missing claim, non-string array entries, a JSON number/object/bool)
/// yields an empty set rather than an error: a token that doesn't carry a
/// usable claim conveys no memberships/roles from it, the same "deny by
/// default" a `NotAuthorized` from an empty static-token tenant list already
/// means. Shared by both the tenant claim (`validate`/`subject`) and the
/// roles claim (`subject` only, `#34`) — the read rule is identical for
/// either.
fn string_set_from_claim(claims: &serde_json::Value, claim_name: &str) -> HashSet<String> {
    match claims.get(claim_name) {
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(|value| value.as_str())
            .map(str::to_string)
            .collect(),
        Some(serde_json::Value::String(value)) => {
            value.split_whitespace().map(str::to_string).collect()
        }
        _ => HashSet::new(),
    }
}

/// One JWKS key, resolved to the form `jsonwebtoken::decode` needs plus the
/// algorithm this validator has decided the key is for (see
/// [`algorithm_for_jwk`]) — checked again against the token header's `alg`
/// by [`OidcValidator::validate`] before the key is ever used.
#[derive(Clone)]
struct CachedKey {
    decoding_key: DecodingKey,
    algorithm: Algorithm,
}

/// In-process JWKS cache with a bounded, single-flight refresh (`#34`).
///
/// A cache is either *fresh* (a fetch — successful or not — was attempted
/// within the last `ttl`) or *stale* (no attempt yet, or the last one was
/// more than `ttl` ago). [`resolve`](Self::resolve) never makes more than
/// one refresh attempt per `ttl` window, regardless of how many distinct
/// `kid`s ask for a key during that window:
///
/// - A fresh cache answers every lookup — hit or miss — from memory alone,
///   with no lock beyond the cache's own read lock and no network call. An
///   unknown `kid` against a fresh cache is rejected immediately.
/// - A stale cache triggers exactly one refresh, gated by `refresh_gate`
///   (a `tokio::sync::Mutex`): every concurrent caller that also observes a
///   stale cache queues on that gate rather than firing its own fetch, and
///   re-checks freshness after acquiring it (the holder that actually ran
///   the fetch has, by then, already made the cache fresh again).
///
/// This is the load-bearing property against an attacker (or a buggy
/// client) sending a flood of unknown/random `kid`s: after the one refresh
/// attempt a burst like that triggers, every further request in the same
/// `ttl` window is a fresh-cache miss — no lock contention, no additional
/// request to the identity provider — until the window rolls over. A
/// refresh that fails (issuer unreachable, non-200, bad JSON, ...) still
/// counts as "attempted": it still sets the fresh-until clock, so a
/// persistently-down issuer degrades to "every token rejected as unknown
/// `kid`, at most one upstream request per TTL window," never to a
/// request storm against a struggling identity provider.
struct JwksCache {
    ttl: Duration,
    state: RwLock<JwksState>,
    refresh_gate: Mutex<()>,
}

struct JwksState {
    keys: HashMap<String, CachedKey>,
    /// `None` until the first refresh attempt (successful or not) — the
    /// cold-start case every freshly built validator (including one built
    /// by a config reload, per `OidcValidator::new`'s own doc) starts in.
    last_attempt_at: Option<Instant>,
}

/// The outcome of a single freshness-aware cache lookup — see
/// [`JwksCache::lookup`].
enum Lookup {
    Found(CachedKey),
    /// The cache is fresh, but this `kid` isn't in it: a final answer, not
    /// a reason to refresh.
    FreshMiss,
    /// The cache needs a refresh attempt before this `kid` can be
    /// answered either way.
    Stale,
}

impl JwksCache {
    fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            state: RwLock::new(JwksState {
                keys: HashMap::new(),
                last_attempt_at: None,
            }),
            refresh_gate: Mutex::new(()),
        }
    }

    async fn lookup(&self, kid: &str) -> Lookup {
        let state = self.state.read().await;
        let fresh = state
            .last_attempt_at
            .is_some_and(|attempted_at| attempted_at.elapsed() < self.ttl);
        match (fresh, state.keys.get(kid)) {
            (true, Some(key)) => Lookup::Found(key.clone()),
            (true, None) => Lookup::FreshMiss,
            (false, _) => Lookup::Stale,
        }
    }

    async fn resolve(
        &self,
        http: &reqwest::Client,
        issuer: &str,
        kid: &str,
    ) -> Result<CachedKey, OidcError> {
        match self.lookup(kid).await {
            Lookup::Found(key) => return Ok(key),
            Lookup::FreshMiss => return Err(OidcError::UnknownKid),
            Lookup::Stale => {}
        }

        // Single-flight: whoever gets here first actually refreshes; every
        // other concurrent caller queues on this gate instead of also
        // firing a fetch, then re-checks freshness once it's their turn —
        // by construction that's always `Found` or `FreshMiss` by then,
        // since the refresh (success or failure) always bumps
        // `last_attempt_at` before releasing the gate.
        let _permit = self.refresh_gate.lock().await;
        match self.lookup(kid).await {
            Lookup::Found(key) => return Ok(key),
            Lookup::FreshMiss => return Err(OidcError::UnknownKid),
            Lookup::Stale => {}
        }

        self.refresh(http, issuer).await;
        self.state
            .read()
            .await
            .keys
            .get(kid)
            .cloned()
            .ok_or(OidcError::UnknownKid)
    }

    async fn refresh(&self, http: &reqwest::Client, issuer: &str) {
        let attempted_at = Instant::now();
        let fetched = fetch_jwks(http, issuer).await;
        let mut state = self.state.write().await;
        // Bump the fresh-until clock unconditionally, success or failure —
        // see this type's own doc for why a failed attempt still counts.
        state.last_attempt_at = Some(attempted_at);
        if let Ok(keys) = fetched {
            state.keys = keys;
        }
    }
}

/// The subset of an OIDC discovery document (`/.well-known/
/// openid-configuration`) this validator reads. Every other field is
/// ignored by `serde`'s default "unknown fields are fine" behavior.
#[derive(Debug, serde::Deserialize)]
struct DiscoveryDocument {
    jwks_uri: String,
}

fn validated_oidc_endpoint_url(raw: &str) -> Result<url::Url, ()> {
    let url = url::Url::parse(raw).map_err(|_| ())?;
    oidc_endpoint_url_is_allowed(&url).then_some(url).ok_or(())
}

fn oidc_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many OIDC endpoint redirects");
        }
        if oidc_endpoint_url_is_allowed(attempt.url()) {
            attempt.follow()
        } else {
            attempt.error("OIDC redirect target must use HTTPS or exact loopback HTTP")
        }
    })
}

/// Discovers `jwks_uri` from `issuer`'s well-known document, then fetches
/// and parses the JWKS itself into the map [`JwksCache::resolve`] indexes
/// by `kid`. A JWK this validator can't use (missing `kid`, an algorithm
/// family/curve other than RSA or P-256, or one `DecodingKey::from_jwk`
/// itself rejects) is skipped rather than failing the whole fetch — see
/// [`algorithm_for_jwk`]'s own doc.
async fn fetch_jwks(
    http: &reqwest::Client,
    issuer: &str,
) -> Result<HashMap<String, CachedKey>, ()> {
    let discovery_url = validated_oidc_endpoint_url(&format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    ))?;
    let discovery: DiscoveryDocument = http
        .get(discovery_url)
        .send()
        .await
        .map_err(|_| ())?
        .error_for_status()
        .map_err(|_| ())?
        .json()
        .await
        .map_err(|_| ())?;

    let jwks_url = validated_oidc_endpoint_url(&discovery.jwks_uri)?;
    let jwk_set: JwkSet = http
        .get(jwks_url)
        .send()
        .await
        .map_err(|_| ())?
        .error_for_status()
        .map_err(|_| ())?
        .json()
        .await
        .map_err(|_| ())?;

    Ok(jwk_set
        .keys
        .into_iter()
        .filter_map(|jwk| {
            let kid = jwk.common.key_id.clone()?;
            let algorithm = algorithm_for_jwk(&jwk)?;
            let decoding_key = DecodingKey::from_jwk(&jwk).ok()?;
            Some((
                kid,
                CachedKey {
                    decoding_key,
                    algorithm,
                },
            ))
        })
        .collect())
}

/// The [`Algorithm`] a JWK implies, derived from its key type (and, for EC
/// keys, curve) rather than trusted from the JWK's own optional `alg`
/// field — many identity providers omit it. RSA keys are always treated as
/// RS256 (this validator doesn't support RS384/RS512/PS*); EC keys are
/// ES256 only when the curve is P-256. Every other `kty`/`crv` combination
/// (HMAC octet keys, Ed25519, P-384, P-521) returns `None`, so
/// [`fetch_jwks`] simply skips that key rather than mis-selecting an
/// algorithm for it.
fn algorithm_for_jwk(jwk: &Jwk) -> Option<Algorithm> {
    match &jwk.algorithm {
        AlgorithmParameters::RSA(_) => Some(Algorithm::RS256),
        AlgorithmParameters::EllipticCurve(params) if params.curve == EllipticCurve::P256 => {
            Some(Algorithm::ES256)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn no_credential_is_denied_with_no_credential_reason() {
        let authorizer =
            StaticBearerAuthorizer::new([("token-a".to_string(), vec!["tenant-a".to_string()])]);
        let decision = authorizer.authorize(&Credential::None, "tenant-a").await;
        assert_eq!(decision, AuthDecision::Deny(DenyReason::NoCredential));
    }

    #[tokio::test]
    async fn an_unknown_token_is_denied_as_not_authorized() {
        let authorizer =
            StaticBearerAuthorizer::new([("token-a".to_string(), vec!["tenant-a".to_string()])]);
        let decision = authorizer
            .authorize(&Credential::Bearer("no-such-token".to_string()), "tenant-a")
            .await;
        assert_eq!(decision, AuthDecision::Deny(DenyReason::NotAuthorized));
    }

    #[tokio::test]
    async fn a_known_token_for_a_different_tenant_is_denied_as_not_authorized() {
        let authorizer =
            StaticBearerAuthorizer::new([("token-a".to_string(), vec!["tenant-a".to_string()])]);
        let decision = authorizer
            .authorize(&Credential::Bearer("token-a".to_string()), "tenant-b")
            .await;
        assert_eq!(decision, AuthDecision::Deny(DenyReason::NotAuthorized));
    }

    #[tokio::test]
    async fn a_token_authorizing_the_target_tenant_is_allowed() {
        let authorizer = StaticBearerAuthorizer::new([(
            "token-a".to_string(),
            vec!["tenant-a".to_string(), "tenant-b".to_string()],
        )]);
        assert_eq!(
            authorizer
                .authorize(&Credential::Bearer("token-a".to_string()), "tenant-a")
                .await,
            AuthDecision::Allow
        );
        assert_eq!(
            authorizer
                .authorize(&Credential::Bearer("token-a".to_string()), "tenant-b")
                .await,
            AuthDecision::Allow
        );
    }

    #[tokio::test]
    async fn distinct_tokens_authorize_distinct_tenants_independently() {
        let authorizer = StaticBearerAuthorizer::new([
            ("token-a".to_string(), vec!["tenant-a".to_string()]),
            ("token-b".to_string(), vec!["tenant-b".to_string()]),
        ]);
        assert_eq!(
            authorizer
                .authorize(&Credential::Bearer("token-b".to_string()), "tenant-a")
                .await,
            AuthDecision::Deny(DenyReason::NotAuthorized),
            "tenant-b's token must not authorize tenant-a"
        );
        assert_eq!(
            authorizer
                .authorize(&Credential::Bearer("token-b".to_string()), "tenant-b")
                .await,
            AuthDecision::Allow
        );
    }

    #[test]
    fn credential_debug_never_prints_the_bearer_token_value() {
        let credential = Credential::Bearer("super-secret-token-value".to_string());
        let debugged = format!("{credential:?}");
        assert!(!debugged.contains("super-secret-token-value"));
    }

    #[test]
    fn build_authorizer_is_none_for_the_default_permissive_config() {
        assert!(build_authorizer(&AuthConfig::default())
            .expect("the permissive default resolves no credentials at all")
            .is_none());
    }

    #[tokio::test]
    async fn build_authorizer_builds_a_working_static_authorizer_from_config() {
        let auth = AuthConfig {
            bearer_tokens: vec![crate::config::BearerTokenDecl {
                token: "cfg-token".to_string(),
                tenants: vec!["tenant-a".to_string()],
                ..Default::default()
            }],
            oidc: None,
            trusted_issuers: Vec::new(),
            browser: None,
        };
        let authorizer = build_authorizer(&auth)
            .expect("an inline token needs no environment")
            .expect("static backend builds an authorizer");
        assert_eq!(
            authorizer
                .authorize(&Credential::Bearer("cfg-token".to_string()), "tenant-a")
                .await,
            AuthDecision::Allow
        );
    }

    // -- platform-admin authorization (`#110`) -------------------------

    #[tokio::test]
    async fn no_credential_is_denied_platform_admin_with_no_credential_reason() {
        let authorizer =
            StaticBearerAuthorizer::new([("token-a".to_string(), vec!["tenant-a".to_string()])]);
        assert_eq!(
            authorizer.authorize_platform_admin(&Credential::None).await,
            PlatformAdminDecision::Deny(DenyReason::NoCredential)
        );
    }

    #[tokio::test]
    async fn a_token_without_platform_admin_is_denied_as_not_authorized() {
        let auth = AuthConfig {
            bearer_tokens: vec![crate::config::BearerTokenDecl {
                token: "cfg-token".to_string(),
                tenants: vec!["tenant-a".to_string()],
                platform_admin: false,
                ..Default::default()
            }],
            oidc: None,
            trusted_issuers: Vec::new(),
            browser: None,
        };
        let authorizer = build_authorizer(&auth)
            .expect("an inline token needs no environment")
            .unwrap();
        assert_eq!(
            authorizer
                .authorize_platform_admin(&Credential::Bearer("cfg-token".to_string()))
                .await,
            PlatformAdminDecision::Deny(DenyReason::NotAuthorized)
        );
    }

    #[tokio::test]
    async fn a_platform_admin_token_is_allowed_with_its_declared_principal() {
        let auth = AuthConfig {
            bearer_tokens: vec![crate::config::BearerTokenDecl {
                token: "admin-token".to_string(),
                tenants: vec!["tenant-a".to_string()],
                platform_admin: true,
                principal: Some("carlo".to_string()),
                ..Default::default()
            }],
            oidc: None,
            trusted_issuers: Vec::new(),
            browser: None,
        };
        let authorizer = build_authorizer(&auth)
            .expect("an inline token needs no environment")
            .unwrap();
        assert_eq!(
            authorizer
                .authorize_platform_admin(&Credential::Bearer("admin-token".to_string()))
                .await,
            PlatformAdminDecision::Allow {
                principal: "carlo".to_string()
            }
        );
    }

    #[tokio::test]
    async fn an_exact_durable_static_sysadmin_binding_authorizes_its_token() {
        let auth = AuthConfig {
            bearer_tokens: vec![crate::config::BearerTokenDecl {
                token: "bound-token".to_string(),
                tenants: vec!["tenant-a".to_string()],
                platform_admin: false,
                principal: Some("recovery-operator".to_string()),
                ..Default::default()
            }],
            oidc: None,
            trusted_issuers: Vec::new(),
            browser: None,
        };
        let exact = RoleBinding {
            principal: PrincipalIdentity {
                issuer: "urn:tellurion:static".to_string(),
                subject: "recovery-operator".to_string(),
            },
            role: "sysadmin".to_string(),
            scope: ControlScope::Platform,
        };
        let authorizer = build_authorizer_with_bindings(&auth, &[exact])
            .unwrap()
            .unwrap();
        assert_eq!(
            authorizer
                .authorize_platform_admin(&Credential::Bearer("bound-token".to_string()))
                .await,
            PlatformAdminDecision::Allow {
                principal: "recovery-operator".to_string()
            }
        );

        let mismatched = RoleBinding {
            principal: PrincipalIdentity {
                issuer: "urn:tellurion:static".to_string(),
                subject: "someone-else".to_string(),
            },
            role: "sysadmin".to_string(),
            scope: ControlScope::Platform,
        };
        let authorizer = build_authorizer_with_bindings(&auth, &[mismatched])
            .unwrap()
            .unwrap();
        assert_eq!(
            authorizer
                .authorize_platform_admin(&Credential::Bearer("bound-token".to_string()))
                .await,
            PlatformAdminDecision::Deny(DenyReason::NotAuthorized)
        );
    }

    /// A platform-admin token with no declared `principal` still authorizes,
    /// falling back to a short, non-reversible fingerprint — never the raw
    /// token value itself.
    #[tokio::test]
    async fn a_platform_admin_token_with_no_declared_principal_falls_back_to_a_fingerprint() {
        let auth = AuthConfig {
            bearer_tokens: vec![crate::config::BearerTokenDecl {
                token: "unnamed-admin-token".to_string(),
                tenants: vec!["tenant-a".to_string()],
                platform_admin: true,
                ..Default::default()
            }],
            oidc: None,
            trusted_issuers: Vec::new(),
            browser: None,
        };
        let authorizer = build_authorizer(&auth)
            .expect("an inline token needs no environment")
            .unwrap();
        match authorizer
            .authorize_platform_admin(&Credential::Bearer("unnamed-admin-token".to_string()))
            .await
        {
            PlatformAdminDecision::Allow { principal } => {
                assert!(principal.starts_with("token:"));
                assert!(
                    !principal.contains("unnamed-admin-token"),
                    "the fallback principal must never contain the raw token value"
                );
            }
            other => panic!("expected Allow, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_unknown_token_is_denied_platform_admin_as_not_authorized() {
        let auth = AuthConfig {
            bearer_tokens: vec![crate::config::BearerTokenDecl {
                token: "some-token".to_string(),
                tenants: vec!["tenant-a".to_string()],
                platform_admin: true,
                ..Default::default()
            }],
            oidc: None,
            trusted_issuers: Vec::new(),
            browser: None,
        };
        let authorizer = build_authorizer(&auth)
            .expect("an inline token needs no environment")
            .unwrap();
        assert_eq!(
            authorizer
                .authorize_platform_admin(&Credential::Bearer("no-such-token".to_string()))
                .await,
            PlatformAdminDecision::Deny(DenyReason::NotAuthorized)
        );
    }

    // -- credential storage seam (`#144`) -------------------------------

    /// The secret this suite watches for. Distinctive enough that any
    /// rendering of it anywhere is unambiguous.
    const ENV_SECRET: &str = "s3cret-value-from-the-environment";

    fn env_principal(var: &str, tenant: &str) -> AuthConfig {
        AuthConfig {
            bearer_tokens: vec![crate::config::BearerTokenDecl {
                token_env: Some(var.to_string()),
                tenants: vec![tenant.to_string()],
                principal: Some("service-account".to_string()),
                ..Default::default()
            }],
            oidc: None,
            trusted_issuers: Vec::new(),
            browser: None,
        }
    }

    #[test]
    fn a_token_env_principal_resolves_the_value_the_environment_holds() {
        let auth = env_principal("TELLURION_BEARER_A", "tenant-a");
        let resolved = resolve_bearer_credentials_from(&auth, |name| {
            (name == "TELLURION_BEARER_A").then(|| ENV_SECRET.to_string())
        })
        .expect("a set variable resolves");
        assert_eq!(resolved.len(), 1);
        // Nothing inline, so nothing to warn about.
        assert!(resolved.inline_principals().is_empty());
        assert!(resolved.inline_credential_warning().is_none());
    }

    /// The decisive negative (`#144`): a token value read from the
    /// environment must not be reachable through any rendering of the types
    /// that carry it — not the `Debug` of the resolved set, not the `Debug`
    /// of a credential, and not the message of any error this seam can
    /// produce. Every string a log line or a response body could be built
    /// from is checked against the one value that must never appear in one.
    #[test]
    fn no_rendering_of_a_resolved_credential_ever_contains_the_token_value() {
        let auth = env_principal("TELLURION_BEARER_B", "tenant-a");
        let lookup = |_: &str| Some(ENV_SECRET.to_string());

        let resolved = resolve_bearer_credentials_from(&auth, lookup).expect("resolves");
        let debugged = format!("{resolved:?}");
        assert!(
            !debugged.contains(ENV_SECRET),
            "the resolved-credential Debug leaked the token: {debugged}"
        );

        // The same value, presented as a request credential.
        let credential = Credential::Bearer(ENV_SECRET.to_string());
        assert!(!format!("{credential:?}").contains(ENV_SECRET));

        // Every error this seam can produce, rendered the way
        // `PUT /config?dry_run=true` renders one into a response body and
        // `main.rs` renders one into a boot log line.
        let mut two_reading_one_value = env_principal("TELLURION_BEARER_B", "tenant-a");
        two_reading_one_value
            .bearer_tokens
            .push(crate::config::BearerTokenDecl {
                token_env: Some("TELLURION_BEARER_C".to_string()),
                tenants: vec!["tenant-a".to_string()],
                ..Default::default()
            });
        let messages = [
            resolve_bearer_credentials_from(&auth, |_| None)
                .expect_err("an unset variable refuses")
                .to_string(),
            resolve_bearer_credentials_from(&auth, |_| Some(String::new()))
                .expect_err("an empty variable refuses")
                .to_string(),
            resolve_bearer_credentials_from(&two_reading_one_value, lookup)
                .expect_err("two entries resolving to one value refuse")
                .to_string(),
        ];
        for message in messages {
            assert!(
                !message.contains(ENV_SECRET),
                "an error message leaked the token: {message}"
            );
        }
    }

    /// A refusal is named, and names the one thing that fixes it: the
    /// variable. Silence — a principal that simply stops authorizing — is
    /// the outcome this rules out.
    #[test]
    fn an_unset_token_env_is_refused_by_name() {
        let auth = env_principal("TELLURION_BEARER_UNSET", "tenant-a");
        let error = resolve_bearer_credentials_from(&auth, |_| None)
            .expect_err("an unset variable must refuse, not skip the principal");
        let message = error.to_string();
        assert!(message.contains("auth.bearer_tokens"), "{message}");
        assert!(message.contains("TELLURION_BEARER_UNSET"), "{message}");
        assert!(message.contains("is not set"), "{message}");
    }

    /// A set-but-empty variable is the same refusal, not an empty token
    /// that would authorize an `Authorization: Bearer ` header.
    #[test]
    fn an_empty_token_env_is_refused_by_name() {
        let auth = env_principal("TELLURION_BEARER_EMPTY", "tenant-a");
        let error = resolve_bearer_credentials_from(&auth, |_| Some(String::new()))
            .expect_err("an empty variable must refuse");
        assert!(error.to_string().contains("TELLURION_BEARER_EMPTY"));
    }

    /// The pre-`#144` document: it still boots, still authorizes exactly as
    /// it did, and is reported by name — with the principal named and the
    /// value nowhere in the report.
    #[test]
    fn an_inline_token_still_works_and_is_reported_by_name() {
        let auth = AuthConfig {
            bearer_tokens: vec![
                crate::config::BearerTokenDecl {
                    token: "legacy-inline-token".to_string(),
                    tenants: vec!["tenant-a".to_string()],
                    principal: Some("legacy-service".to_string()),
                    ..Default::default()
                },
                crate::config::BearerTokenDecl {
                    token: "another-legacy-token".to_string(),
                    tenants: vec!["tenant-a".to_string()],
                    ..Default::default()
                },
            ],
            oidc: None,
            trusted_issuers: Vec::new(),
            browser: None,
        };
        let resolved = resolve_bearer_credentials_from(&auth, |_| {
            panic!("an inline token must never consult the environment")
        })
        .expect("an inline token resolves to itself");
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved.inline_principals().len(), 2);

        let warning = resolved
            .inline_credential_warning()
            .expect("an inline credential must be reported");
        assert!(warning.contains("auth.bearer_tokens"), "{warning}");
        assert!(warning.contains("token_env"), "{warning}");
        assert!(warning.contains("(#144)"), "{warning}");
        // The principal that named itself is named; the one that did not is
        // a fingerprint. Neither is a token value.
        assert!(warning.contains("legacy-service"), "{warning}");
        assert!(warning.contains("token:"), "{warning}");
        assert!(!warning.contains("legacy-inline-token"), "{warning}");
        assert!(!warning.contains("another-legacy-token"), "{warning}");
    }

    /// The permissive default resolves nothing and says nothing — an
    /// unconfigured deployment gains no new output from this slice.
    #[test]
    fn the_unconfigured_default_resolves_no_credentials_and_warns_about_nothing() {
        let resolved = resolve_bearer_credentials_from(&AuthConfig::default(), |_| {
            panic!("no principal, no lookup")
        })
        .expect("the permissive default resolves");
        assert!(resolved.is_empty());
        assert!(resolved.inline_credential_warning().is_none());
    }

    /// End to end through the real process environment, the way `main.rs`
    /// resolves at boot: an authorizer built from a `token_env` principal
    /// authorizes the value that variable holds, and nothing else.
    #[tokio::test]
    async fn an_authorizer_built_from_a_token_env_authorizes_the_environments_value() {
        // A variable name unique to this test: the process environment is
        // shared by every test in this binary.
        const VAR: &str = "TELLURION_TEST_BEARER_144_END_TO_END";
        std::env::set_var(VAR, ENV_SECRET);
        let authorizer = build_authorizer(&env_principal(VAR, "tenant-a"))
            .expect("a set variable resolves")
            .expect("a configured auth section builds an authorizer");
        assert_eq!(
            authorizer
                .authorize(&Credential::Bearer(ENV_SECRET.to_string()), "tenant-a")
                .await,
            AuthDecision::Allow
        );
        assert_eq!(
            authorizer
                .authorize(&Credential::Bearer(VAR.to_string()), "tenant-a")
                .await,
            AuthDecision::Deny(DenyReason::NotAuthorized),
            "the variable NAME is not the credential"
        );
        std::env::remove_var(VAR);
    }

    // --- OIDC (`#34`) ------------------------------------------------
    //
    // These tests generate a fresh RSA keypair per test (never a checked-in
    // fixture — see the module's own clean-room rule) and serve the JWKS
    // from a real local axum server, so `OidcValidator` exercises its own
    // HTTP fetch + JWKS parsing path exactly as it would against a real
    // identity provider. `oidc_test_support` below hosts the shared
    // scaffolding.
    mod oidc_test_support {
        use std::net::SocketAddr;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use axum::extract::State;
        use axum::response::Json;
        use axum::routing::get;
        use jsonwebtoken::jwk::{Jwk, JwkSet};
        use jsonwebtoken::{Algorithm, EncodingKey, Header};
        use rsa::pkcs1::EncodeRsaPrivateKey;
        use rsa::RsaPrivateKey;
        use serde_json::json;

        use crate::config::{OidcClaimsConfig, OidcConfig};

        /// A freshly generated RSA keypair plus the `EncodingKey`
        /// `jsonwebtoken` needs to sign with it — generated in-process,
        /// never loaded from a checked-in file.
        pub struct TestKeyPair {
            pub encoding_key: EncodingKey,
        }

        pub fn generate_rsa_keypair() -> TestKeyPair {
            let mut rng = rand::thread_rng();
            let private_key =
                RsaPrivateKey::new(&mut rng, 2048).expect("generate a 2048-bit RSA test key");
            let der = private_key
                .to_pkcs1_der()
                .expect("encode the test private key as PKCS#1 DER");
            TestKeyPair {
                encoding_key: EncodingKey::from_rsa_der(der.as_bytes()),
            }
        }

        /// Mints a JWT signed by `key`, with `kid` in the header and
        /// `claims` as the payload — a thin wrapper so every test builds a
        /// token the same way and only varies the claims/kid/key under
        /// test.
        pub fn sign_token(key: &TestKeyPair, kid: &str, claims: &serde_json::Value) -> String {
            let mut header = Header::new(Algorithm::RS256);
            header.kid = Some(kid.to_string());
            jsonwebtoken::encode(&header, claims, &key.encoding_key).expect("sign a test token")
        }

        /// A running fake JWKS endpoint: serves `/.well-known/
        /// openid-configuration` (pointing back at its own `/jwks`) and
        /// `/jwks` itself, counting how many times `/jwks` was actually
        /// fetched — enough for the single-flight/TTL tests to assert on
        /// fetch count, not just outcome.
        pub struct FakeIdp {
            pub issuer: String,
            pub fetch_count: std::sync::Arc<AtomicUsize>,
            pub jwks: std::sync::Arc<tokio::sync::RwLock<serde_json::Value>>,
            _shutdown: tokio::sync::oneshot::Sender<()>,
        }

        #[derive(Clone)]
        struct FakeIdpState {
            jwks: std::sync::Arc<tokio::sync::RwLock<serde_json::Value>>,
            jwks_uri: String,
            fetch_count: std::sync::Arc<AtomicUsize>,
        }

        /// Starts a fake IdP on an ephemeral local port serving exactly one
        /// RSA key (`kid` = `"test-kid"`, matching `sign_token`'s default
        /// unless the caller overrides it in the token header directly).
        /// Binds the listener before building the router so the discovery
        /// document's `jwks_uri` (which needs to know this server's own
        /// address) is a plain, already-known `String` in `FakeIdpState`
        /// rather than something request handlers have to compute.
        pub async fn start_fake_idp(key: &TestKeyPair, kid: &str) -> FakeIdp {
            let jwks_json = jwks_for(key, kid);

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind an ephemeral port for the fake IdP");
            let addr: SocketAddr = listener.local_addr().unwrap();
            let issuer = format!("http://{addr}");

            let fetch_count = std::sync::Arc::new(AtomicUsize::new(0));
            let jwks = std::sync::Arc::new(tokio::sync::RwLock::new(jwks_json));
            let state = FakeIdpState {
                jwks: std::sync::Arc::clone(&jwks),
                jwks_uri: format!("{issuer}/jwks"),
                fetch_count: std::sync::Arc::clone(&fetch_count),
            };

            let app = axum::Router::new()
                .route(
                    "/.well-known/openid-configuration",
                    get(|State(state): State<FakeIdpState>| async move {
                        Json(json!({ "jwks_uri": state.jwks_uri }))
                    }),
                )
                .route(
                    "/jwks",
                    get(|State(state): State<FakeIdpState>| async move {
                        state.fetch_count.fetch_add(1, Ordering::SeqCst);
                        Json(state.jwks.read().await.clone())
                    }),
                )
                .with_state(state);

            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = shutdown_rx.await;
                    })
                    .await
                    .expect("fake IdP server task");
            });

            FakeIdp {
                issuer,
                fetch_count,
                jwks,
                _shutdown: shutdown_tx,
            }
        }

        pub fn jwks_for(key: &TestKeyPair, kid: &str) -> serde_json::Value {
            let mut jwk = Jwk::from_encoding_key(&key.encoding_key, Algorithm::RS256)
                .expect("derive a public JWK from the test signing key");
            jwk.common.key_id = Some(kid.to_string());
            let jwk_set = JwkSet { keys: vec![jwk] };
            serde_json::to_value(&jwk_set).expect("serialize the test JWKS")
        }

        pub fn oidc_config(issuer: &str) -> OidcConfig {
            OidcConfig {
                issuer: issuer.to_string(),
                audience: "tellurion-test".to_string(),
                claims: OidcClaimsConfig {
                    tenants: "tenants".to_string(),
                    ..Default::default()
                },
                claims_authoritative: false,
                clock_skew_s: 5,
                jwks_ttl_s: 300,
            }
        }
    }

    use oidc_test_support::*;
    use std::sync::atomic::Ordering;

    use crate::control_model::{ControlScope, PrincipalIdentity, RoleBinding};
    use crate::identity::TrustedIssuerSet;

    fn unix_now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[test]
    fn oidc_network_endpoints_require_https_except_for_exact_loopback_hosts() {
        for endpoint in [
            "https://idp.example.com/jwks",
            "http://127.0.0.1:8080/jwks",
            "http://[::1]:8080/jwks",
            "http://localhost:8080/jwks",
        ] {
            assert!(validated_oidc_endpoint_url(endpoint).is_ok(), "{endpoint}");
        }
        for endpoint in [
            "http://idp.example.com/jwks",
            "http://localhost.example.com/jwks",
            "file:///tmp/jwks.json",
        ] {
            assert!(validated_oidc_endpoint_url(endpoint).is_err(), "{endpoint}");
        }
    }

    #[tokio::test]
    async fn oidc_http_client_rejects_redirects_to_remote_plain_http() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind redirect test server");
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().route(
            "/redirect",
            axum::routing::get(|| async {
                axum::response::Redirect::temporary("http://idp.example.com/jwks")
            }),
        );
        let server =
            tokio::spawn(async move { axum::serve(listener, app).await.expect("serve redirect") });
        let validator = OidcValidator::new(oidc_config(&issuer));

        let error = validator
            .http
            .get(format!("{issuer}/redirect"))
            .send()
            .await
            .expect_err("remote plaintext redirect must be blocked");

        assert!(error.is_redirect(), "{error:?}");
        server.abort();
    }

    #[tokio::test]
    async fn discovery_rejects_a_remote_plain_http_jwks_uri_before_fetching_it() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind discovery test server");
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().route(
            "/.well-known/openid-configuration",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "jwks_uri": "http://192.0.2.1/jwks"
                }))
            }),
        );
        let server =
            tokio::spawn(async move { axum::serve(listener, app).await.expect("serve discovery") });
        let validator = OidcValidator::new(oidc_config(&issuer));

        let result = tokio::time::timeout(
            Duration::from_millis(500),
            fetch_jwks(&validator.http, &issuer),
        )
        .await
        .expect("unsafe JWKS URL must be rejected before a network timeout");

        assert!(result.is_err());
        server.abort();
    }

    #[tokio::test]
    async fn trusted_issuer_set_selects_the_exact_issuer_and_requires_sub() {
        let key_a = generate_rsa_keypair();
        let key_b = generate_rsa_keypair();
        let idp_a = start_fake_idp(&key_a, "kid-a").await;
        let idp_b = start_fake_idp(&key_b, "kid-b").await;
        let issuers =
            TrustedIssuerSet::new([oidc_config(&idp_a.issuer), oidc_config(&idp_b.issuer)]);

        let claims = serde_json::json!({
            "iss": idp_b.issuer,
            "sub": "operator-42",
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
        });
        let token = sign_token(&key_b, "kid-b", &claims);
        let authenticated = issuers
            .authenticate(&token)
            .await
            .expect("token authenticates");
        assert_eq!(
            authenticated.principal,
            PrincipalIdentity {
                issuer: idp_b.issuer.clone(),
                subject: "operator-42".to_string(),
            }
        );
        assert_eq!(idp_a.fetch_count.load(Ordering::SeqCst), 0);
        assert_eq!(idp_b.fetch_count.load(Ordering::SeqCst), 1);

        let claims_a = serde_json::json!({
            "iss": idp_a.issuer,
            "sub": "operator-a",
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
        });
        let token_a = sign_token(&key_a, "kid-a", &claims_a);
        issuers
            .authenticate(&token_a)
            .await
            .expect("issuer A authenticates");
        assert_eq!(idp_a.fetch_count.load(Ordering::SeqCst), 1);
        assert_eq!(idp_b.fetch_count.load(Ordering::SeqCst), 1);

        let claims_without_sub = serde_json::json!({
            "iss": idp_b.issuer,
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
        });
        let token_without_sub = sign_token(&key_b, "kid-b", &claims_without_sub);
        assert!(issuers.authenticate(&token_without_sub).await.is_err());
    }

    #[tokio::test]
    async fn one_trusted_issuer_refreshes_its_rotated_key_after_ttl() {
        let first_key = generate_rsa_keypair();
        let second_key = generate_rsa_keypair();
        let idp = start_fake_idp(&first_key, "kid-1").await;
        let mut config = oidc_config(&idp.issuer);
        config.jwks_ttl_s = 1;
        let issuers = TrustedIssuerSet::new([config]);
        let claims = serde_json::json!({
            "iss": idp.issuer,
            "sub": "operator",
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
        });
        let first_token = sign_token(&first_key, "kid-1", &claims);
        issuers
            .authenticate(&first_token)
            .await
            .expect("first key validates");

        *idp.jwks.write().await = jwks_for(&second_key, "kid-2");
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        let second_token = sign_token(&second_key, "kid-2", &claims);
        issuers
            .authenticate(&second_token)
            .await
            .expect("rotated key validates after refresh");
        assert_eq!(idp.fetch_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_saml_upstream_session_is_accepted_only_as_a_verified_oidc_token() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let issuers = TrustedIssuerSet::new([oidc_config(&idp.issuer)]);
        let claims = serde_json::json!({
            "iss": idp.issuer,
            "sub": "brokered-user",
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "amr": ["saml"],
        });
        let token = sign_token(&key, "test-kid", &claims);
        let subject = issuers
            .authenticate(&token)
            .await
            .expect("OIDC token validates");
        assert_eq!(
            subject.claims.get("amr"),
            Some(&serde_json::json!(["saml"]))
        );
    }

    #[tokio::test]
    async fn an_unknown_issuer_is_rejected_without_fetching_any_trusted_jwks() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let issuers = TrustedIssuerSet::new([oidc_config(&idp.issuer)]);
        let claims = serde_json::json!({
            "iss": "https://unknown-issuer.invalid",
            "sub": "attacker",
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
        });
        let token = sign_token(&key, "test-kid", &claims);

        let error = issuers.authenticate(&token).await.unwrap_err();
        assert!(!format!("{error:?}").contains(&token));
        assert_eq!(idp.fetch_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_stored_platform_sysadmin_binding_authorizes_an_exact_oidc_principal() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let auth = AuthConfig {
            trusted_issuers: vec![oidc_config(&idp.issuer)],
            ..Default::default()
        };
        let binding = RoleBinding {
            principal: PrincipalIdentity {
                issuer: idp.issuer.clone(),
                subject: "carlo".to_string(),
            },
            role: "sysadmin".to_string(),
            scope: ControlScope::Platform,
        };
        let authorizer = build_authorizer_with_bindings(&auth, &[binding])
            .expect("an inline token needs no environment")
            .unwrap();

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "sub": "carlo",
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "roles": ["sysadmin"],
        });
        let token = sign_token(&key, "test-kid", &claims);
        assert_eq!(
            authorizer
                .authorize_platform_admin(&Credential::Bearer(token))
                .await,
            PlatformAdminDecision::Allow {
                principal: format!("{}#carlo", idp.issuer)
            }
        );
    }

    #[tokio::test]
    async fn a_raw_sysadmin_claim_without_a_stored_binding_grants_nothing() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let auth = AuthConfig {
            trusted_issuers: vec![oidc_config(&idp.issuer)],
            ..Default::default()
        };
        let authorizer = build_authorizer_with_bindings(&auth, &[])
            .expect("an inline token needs no environment")
            .unwrap();
        let claims = serde_json::json!({
            "iss": idp.issuer,
            "sub": "carlo",
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "roles": ["sysadmin"],
        });
        let token = sign_token(&key, "test-kid", &claims);

        assert_eq!(
            authorizer
                .authorize_platform_admin(&Credential::Bearer(token))
                .await,
            PlatformAdminDecision::Deny(DenyReason::NotAuthorized)
        );
    }

    #[tokio::test]
    async fn raw_tenant_and_role_claims_are_inert_without_an_explicit_mapping_or_binding() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let auth = AuthConfig {
            trusted_issuers: vec![oidc_config(&idp.issuer)],
            ..Default::default()
        };
        let authorizer = build_authorizer_with_bindings(&auth, &[])
            .expect("an inline token needs no environment")
            .unwrap();
        let claims = serde_json::json!({
            "iss": idp.issuer,
            "sub": "carlo",
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "tenants": ["tenant-a"],
            "roles": ["tenant_admin"],
        });
        let token = sign_token(&key, "test-kid", &claims);

        assert_eq!(
            authorizer
                .authorize(&Credential::Bearer(token.clone()), "tenant-a")
                .await,
            AuthDecision::Deny(DenyReason::NotAuthorized)
        );
        assert!(authorizer
            .subject(&Credential::Bearer(token))
            .await
            .memberships
            .is_empty());
    }

    #[tokio::test]
    async fn explicitly_registered_trusted_issuer_claim_mapping_grants_membership() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let mut issuer = oidc_config(&idp.issuer);
        issuer.claims_authoritative = true;
        let auth = AuthConfig {
            trusted_issuers: vec![issuer],
            ..Default::default()
        };
        let authorizer = build_authorizer_with_bindings(&auth, &[])
            .expect("an inline token needs no environment")
            .unwrap();
        let claims = serde_json::json!({
            "iss": idp.issuer,
            "sub": "carlo",
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "tenants": ["tenant-a"],
        });
        let token = sign_token(&key, "test-kid", &claims);

        assert_eq!(
            authorizer
                .authorize(&Credential::Bearer(token), "tenant-a")
                .await,
            AuthDecision::Allow
        );
    }

    #[tokio::test]
    async fn a_tenant_binding_grants_only_the_exact_issuer_and_subject() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let mut config = oidc_config(&idp.issuer);
        config.claims.tenants = "unused_tenant_claim".to_string();
        let auth = AuthConfig {
            trusted_issuers: vec![config],
            ..Default::default()
        };
        let binding = RoleBinding {
            principal: PrincipalIdentity {
                issuer: idp.issuer.clone(),
                subject: "carlo".to_string(),
            },
            role: "tenant_admin".to_string(),
            scope: ControlScope::Tenant {
                tenant_id: "tenant-a".to_string(),
            },
        };
        let authorizer = build_authorizer_with_bindings(&auth, &[binding])
            .expect("an inline token needs no environment")
            .unwrap();
        let claims = serde_json::json!({
            "iss": idp.issuer,
            "sub": "carlo",
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "tenants": ["attacker-controlled"],
        });
        let token = sign_token(&key, "test-kid", &claims);

        let subject = authorizer.subject(&Credential::Bearer(token)).await;
        assert_eq!(
            subject.memberships.get("tenant-a"),
            Some(&HashSet::from(["tenant_admin".to_string()]))
        );
        assert!(!subject.memberships.contains_key("attacker-controlled"));
        assert_eq!(
            subject.identity,
            Some(PrincipalIdentity {
                issuer: idp.issuer,
                subject: "carlo".to_string(),
            })
        );
    }

    #[tokio::test]
    async fn a_valid_oidc_token_authorizes_its_mapped_tenant() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let validator = OidcValidator::new(oidc_config(&idp.issuer));

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "tenants": ["tenant-a", "tenant-b"],
        });
        let token = sign_token(&key, "test-kid", &claims);

        let memberships = validator.validate(&token).await.expect("token validates");
        assert!(memberships.contains("tenant-a"));
        assert!(memberships.contains("tenant-b"));
        assert!(!memberships.contains("tenant-c"));
    }

    #[tokio::test]
    async fn an_expired_oidc_token_is_rejected() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let validator = OidcValidator::new(oidc_config(&idp.issuer));

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "aud": "tellurion-test",
            "exp": unix_now() - 3600,
            "tenants": ["tenant-a"],
        });
        let token = sign_token(&key, "test-kid", &claims);

        assert_eq!(
            validator.validate(&token).await,
            Err(OidcError::InvalidToken)
        );
    }

    #[tokio::test]
    async fn an_oidc_token_with_the_wrong_audience_is_rejected() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let validator = OidcValidator::new(oidc_config(&idp.issuer));

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "aud": "some-other-service",
            "exp": unix_now() + 300,
            "tenants": ["tenant-a"],
        });
        let token = sign_token(&key, "test-kid", &claims);

        assert_eq!(
            validator.validate(&token).await,
            Err(OidcError::InvalidToken)
        );
    }

    #[tokio::test]
    async fn an_oidc_token_with_the_wrong_issuer_is_rejected() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let validator = OidcValidator::new(oidc_config(&idp.issuer));

        let claims = serde_json::json!({
            "iss": "https://not-the-configured-issuer.example",
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "tenants": ["tenant-a"],
        });
        let token = sign_token(&key, "test-kid", &claims);

        assert_eq!(
            validator.validate(&token).await,
            Err(OidcError::InvalidToken)
        );
    }

    #[tokio::test]
    async fn an_oidc_token_with_a_bad_signature_is_rejected() {
        let key = generate_rsa_keypair();
        let other_key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let validator = OidcValidator::new(oidc_config(&idp.issuer));

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "tenants": ["tenant-a"],
        });
        // Signed with a *different* key than the one published in the
        // fake IdP's JWKS under this `kid` — same `kid`, wrong signature.
        // Both are RS256 RSA keys, so this fails signature verification
        // itself, not the algorithm-family check.
        let token = sign_token(&other_key, "test-kid", &claims);

        assert_eq!(
            validator.validate(&token).await,
            Err(OidcError::InvalidToken)
        );
    }

    #[tokio::test]
    async fn an_oidc_token_with_an_unknown_kid_is_rejected() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let validator = OidcValidator::new(oidc_config(&idp.issuer));

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "tenants": ["tenant-a"],
        });
        let token = sign_token(&key, "no-such-kid", &claims);

        assert_eq!(validator.validate(&token).await, Err(OidcError::UnknownKid));
    }

    /// `#34`'s bounded-refresh requirement: a burst of unknown `kid`s
    /// within one TTL window must cost the fake IdP exactly one `/jwks`
    /// fetch, not one per request.
    #[tokio::test]
    async fn a_burst_of_unknown_kids_triggers_only_one_jwks_refresh() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let validator = OidcValidator::new(oidc_config(&idp.issuer));

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "tenants": ["tenant-a"],
        });

        let mut handles = Vec::new();
        for i in 0..20 {
            let token = sign_token(&key, &format!("random-kid-{i}"), &claims);
            let validator = &validator;
            handles.push(async move { validator.validate(&token).await });
        }
        let results = futures::future::join_all(handles).await;
        assert!(
            results
                .iter()
                .all(|result| *result == Err(OidcError::UnknownKid)),
            "every random kid must be rejected as unknown"
        );
        assert_eq!(
            idp.fetch_count.load(Ordering::SeqCst),
            1,
            "20 unknown kids in one TTL window must cost exactly one JWKS fetch"
        );
    }

    #[tokio::test]
    async fn oidc_claim_mapping_accepts_an_array_of_strings() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let validator = OidcValidator::new(oidc_config(&idp.issuer));

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "tenants": ["tenant-a", "tenant-b"],
        });
        let token = sign_token(&key, "test-kid", &claims);

        let memberships = validator.validate(&token).await.unwrap();
        assert_eq!(
            memberships,
            HashSet::from(["tenant-a".to_string(), "tenant-b".to_string()])
        );
    }

    #[tokio::test]
    async fn oidc_claim_mapping_accepts_a_space_separated_string() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let validator = OidcValidator::new(oidc_config(&idp.issuer));

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "tenants": "tenant-a tenant-b",
        });
        let token = sign_token(&key, "test-kid", &claims);

        let memberships = validator.validate(&token).await.unwrap();
        assert_eq!(
            memberships,
            HashSet::from(["tenant-a".to_string(), "tenant-b".to_string()])
        );
    }

    #[tokio::test]
    async fn oidc_claim_mapping_honors_a_configured_claim_name() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let mut config = oidc_config(&idp.issuer);
        config.claims.tenants = "groups".to_string();
        let validator = OidcValidator::new(config);

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "groups": ["tenant-a"],
            "tenants": ["ignored-because-not-the-configured-claim"],
        });
        let token = sign_token(&key, "test-kid", &claims);

        let memberships = validator.validate(&token).await.unwrap();
        assert_eq!(memberships, HashSet::from(["tenant-a".to_string()]));
    }

    /// `#34`'s decision flow: a static token still wins even when OIDC is
    /// also configured, and never touches the JWKS endpoint.
    #[tokio::test]
    async fn a_static_token_still_authorizes_when_oidc_is_also_configured() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let oidc = Arc::new(OidcValidator::new(oidc_config(&idp.issuer)));
        let authorizer = StaticBearerAuthorizer::with_oidc(
            [("service-token".to_string(), vec!["tenant-a".to_string()])],
            Some(oidc),
        );

        let decision = authorizer
            .authorize(&Credential::Bearer("service-token".to_string()), "tenant-a")
            .await;
        assert_eq!(decision, AuthDecision::Allow);
        assert_eq!(
            idp.fetch_count.load(Ordering::SeqCst),
            0,
            "a static-token match must never touch the JWKS endpoint"
        );
    }

    /// `#34`'s decision flow, the OIDC side: a token that misses the
    /// static map falls through to OIDC and is authorized from its claims.
    #[tokio::test]
    async fn an_oidc_token_authorizes_through_the_authorizer_when_the_static_map_misses() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let oidc = Arc::new(OidcValidator::new(oidc_config(&idp.issuer)));
        let authorizer = StaticBearerAuthorizer::with_oidc(
            [("service-token".to_string(), vec!["tenant-a".to_string()])],
            Some(oidc),
        );

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "tenants": ["tenant-b"],
        });
        let token = sign_token(&key, "test-kid", &claims);

        assert_eq!(
            authorizer
                .authorize(&Credential::Bearer(token.clone()), "tenant-b")
                .await,
            AuthDecision::Allow
        );
        assert_eq!(
            authorizer
                .authorize(&Credential::Bearer(token), "tenant-a")
                .await,
            AuthDecision::Deny(DenyReason::NotAuthorized)
        );
    }

    /// An invalid OIDC token (not just a missing one) must still be a 401
    /// (`NoCredential`), not a 403 — see `DenyReason`'s own doc.
    #[tokio::test]
    async fn an_invalid_oidc_token_is_denied_as_no_credential_not_not_authorized() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let oidc = Arc::new(OidcValidator::new(oidc_config(&idp.issuer)));
        let authorizer = StaticBearerAuthorizer::with_oidc(std::iter::empty(), Some(oidc));

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "aud": "tellurion-test",
            "exp": unix_now() - 60,
            "tenants": ["tenant-a"],
        });
        let expired_token = sign_token(&key, "test-kid", &claims);

        assert_eq!(
            authorizer
                .authorize(&Credential::Bearer(expired_token), "tenant-a")
                .await,
            AuthDecision::Deny(DenyReason::NoCredential)
        );
    }

    /// `#110`: a token that validates fine as an OIDC tenant credential is
    /// still never a platform admin — OIDC has no platform-admin claim
    /// modeled yet, so this must be `NotAuthorized` (403), not `Allow`.
    #[tokio::test]
    async fn a_validly_authenticated_oidc_token_is_denied_platform_admin() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let oidc = Arc::new(OidcValidator::new(oidc_config(&idp.issuer)));
        let authorizer = StaticBearerAuthorizer::with_oidc(std::iter::empty(), Some(oidc));

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "aud": "tellurion-test",
            "exp": unix_now() + 300,
            "tenants": ["tenant-a"],
        });
        let token = sign_token(&key, "test-kid", &claims);

        assert_eq!(
            authorizer
                .authorize_platform_admin(&Credential::Bearer(token))
                .await,
            PlatformAdminDecision::Deny(DenyReason::NotAuthorized)
        );
    }

    /// An invalid OIDC token must be `NoCredential` (401) for platform-admin
    /// too, the same 401-vs-403 split `authorize` already applies.
    #[tokio::test]
    async fn an_invalid_oidc_token_is_denied_platform_admin_as_no_credential() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let oidc = Arc::new(OidcValidator::new(oidc_config(&idp.issuer)));
        let authorizer = StaticBearerAuthorizer::with_oidc(std::iter::empty(), Some(oidc));

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "aud": "tellurion-test",
            "exp": unix_now() - 60,
            "tenants": ["tenant-a"],
        });
        let expired_token = sign_token(&key, "test-kid", &claims);

        assert_eq!(
            authorizer
                .authorize_platform_admin(&Credential::Bearer(expired_token))
                .await,
            PlatformAdminDecision::Deny(DenyReason::NoCredential)
        );
    }

    /// Guard: neither a validation error nor the memberships extracted from
    /// a token ever carry the token's own raw text — the module's "never
    /// logs or echoes" rule extended to the OIDC path. `OidcError` is
    /// data-free by construction (see its own doc), so this asserts that
    /// property holds for every variant this test suite actually produces.
    #[tokio::test]
    async fn oidc_validation_failures_never_carry_the_token_text() {
        let key = generate_rsa_keypair();
        let idp = start_fake_idp(&key, "test-kid").await;
        let validator = OidcValidator::new(oidc_config(&idp.issuer));

        let claims = serde_json::json!({
            "iss": idp.issuer,
            "aud": "tellurion-test",
            "exp": unix_now() - 60,
            "tenants": ["tenant-a"],
        });
        let token = sign_token(&key, "test-kid", &claims);

        let err = validator.validate(&token).await.unwrap_err();
        let rendered = format!("{err:?}");
        assert!(
            !rendered.contains(&token),
            "the rendered error must never contain the token text"
        );
        // A `Deny` built from it carries even less — no `Debug` output of
        // `OidcError` ever reaches a caller outside this module.
        let decision = AuthDecision::Deny(DenyReason::NoCredential);
        assert!(!format!("{decision:?}").contains(&token));
    }
}
