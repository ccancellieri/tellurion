use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Extension, RawQuery};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use rand::Rng as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tellurion_core::auth::Credential;
use tellurion_core::config::OidcConfig;
use tellurion_core::{
    AppContext, AuthenticatedSubject, ControlBrowserAuthConfig, PrincipalIdentity, TrustedIssuerSet,
};
use tokio::sync::Mutex;
use tokio::time::Instant;
use url::Url;

use crate::control_session::{
    ControlBrowserSession, ControlSessionStore, InMemoryControlSessionStore, PendingControlLogin,
};

const CONTROL_SESSION_COOKIE: &str = "tellurion_control_session";
const CONTROL_LOGIN_COOKIE: &str = "tellurion_control_login";
const CONTROL_CALLBACK_PATH: &str = "/_auth/control/callback";
const CONTROL_LOGIN_CLEAR_COOKIE: &str = "tellurion_control_login=; HttpOnly; Secure; SameSite=Lax; Path=/_auth/control/callback; Max-Age=0";
const CSRF_HEADER: &str = "x-tellurion-csrf";
const MAX_OIDC_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_CALLBACK_PARAMETER_BYTES: usize = 2048;
const OIDC_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(crate) struct OidcEndpoints {
    authorization: Url,
    token: Url,
}

pub(crate) struct TokenExchange {
    code: String,
    verifier: String,
    redirect_uri: String,
    client_id: String,
    client_secret: Option<String>,
}

pub(crate) struct OidcTokens {
    access_token: String,
    id_token: String,
    expires_in_s: Option<u64>,
}

#[async_trait]
pub(crate) trait OidcTransport: Send + Sync {
    async fn discover(&self, issuer: &str) -> Result<OidcEndpoints, ()>;
    async fn exchange(&self, endpoint: &Url, request: TokenExchange) -> Result<OidcTokens, ()>;
}

struct ReqwestOidcTransport {
    client: reqwest::Client,
}

impl ReqwestOidcTransport {
    fn new() -> anyhow::Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(OIDC_REQUEST_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }
}

#[async_trait]
impl OidcTransport for ReqwestOidcTransport {
    async fn discover(&self, issuer: &str) -> Result<OidcEndpoints, ()> {
        let discovery = validated_endpoint(&format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        ))?;
        let response = self.client.get(discovery).send().await.map_err(|_| ())?;
        if !response.status().is_success() {
            return Err(());
        }
        let body = bounded_response_body(response).await?;
        parse_discovery_document(&body, issuer)
    }

    async fn exchange(&self, endpoint: &Url, request: TokenExchange) -> Result<OidcTokens, ()> {
        let endpoint = validated_endpoint(endpoint.as_str())?;
        let form = {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            serializer
                .append_pair("grant_type", "authorization_code")
                .append_pair("code", &request.code)
                .append_pair("redirect_uri", &request.redirect_uri)
                .append_pair("client_id", &request.client_id)
                .append_pair("code_verifier", &request.verifier);
            if let Some(secret) = request.client_secret.as_deref() {
                serializer.append_pair("client_secret", secret);
            }
            serializer.finish()
        };
        let response = self
            .client
            .post(endpoint)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form)
            .send()
            .await
            .map_err(|_| ())?;
        if !response.status().is_success() {
            return Err(());
        }
        let body = bounded_response_body(response).await?;
        let document: TokenDocument = serde_json::from_slice(&body).map_err(|_| ())?;
        Ok(OidcTokens {
            access_token: document.access_token,
            id_token: document.id_token,
            expires_in_s: document.expires_in,
        })
    }
}

#[derive(Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
}

#[derive(Deserialize)]
struct TokenDocument {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    id_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

async fn bounded_response_body(mut response: reqwest::Response) -> Result<Vec<u8>, ()> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_OIDC_RESPONSE_BYTES as u64)
    {
        return Err(());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        if body.len().saturating_add(chunk.len()) > MAX_OIDC_RESPONSE_BYTES {
            return Err(());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn parse_discovery_document(body: &[u8], expected_issuer: &str) -> Result<OidcEndpoints, ()> {
    if body.len() > MAX_OIDC_RESPONSE_BYTES {
        return Err(());
    }
    let document: DiscoveryDocument = serde_json::from_slice(body).map_err(|_| ())?;
    if document.issuer != expected_issuer {
        return Err(());
    }
    Ok(OidcEndpoints {
        authorization: validated_endpoint(&document.authorization_endpoint)?,
        token: validated_endpoint(&document.token_endpoint)?,
    })
}

fn validated_endpoint(raw: &str) -> Result<Url, ()> {
    let url = Url::parse(raw).map_err(|_| ())?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(());
    }
    if url.scheme() == "https" {
        return Ok(url);
    }
    if url.scheme() != "http" {
        return Err(());
    }
    match url.host() {
        Some(url::Host::Ipv4(address)) if address.is_loopback() => Ok(url),
        Some(url::Host::Ipv6(address)) if address.is_loopback() => Ok(url),
        Some(url::Host::Domain(domain)) if domain.eq_ignore_ascii_case("localhost") => Ok(url),
        _ => Err(()),
    }
}

#[async_trait]
pub(crate) trait BrowserIdentityVerifier: Send + Sync {
    async fn verify(&self, id_token: &str, nonce: &str) -> Result<PrincipalIdentity, ()>;
}

#[async_trait]
pub(crate) trait ControlCredentialAuthorizer: Send + Sync {
    async fn subject(&self, credential: &Credential) -> Option<AuthenticatedSubject>;
    async fn authorize_platform_admin(&self, credential: &Credential) -> Option<String>;
}

struct TrustedBrowserIdentity {
    issuers: TrustedIssuerSet,
}

#[async_trait]
impl BrowserIdentityVerifier for TrustedBrowserIdentity {
    async fn verify(&self, id_token: &str, nonce: &str) -> Result<PrincipalIdentity, ()> {
        self.issuers
            .authenticate_with_nonce(id_token, nonce)
            .await
            .map_err(|_| ())
    }
}

struct CurrentControlAuthorizer {
    context: std::sync::Weak<AppContext>,
}

#[async_trait]
impl ControlCredentialAuthorizer for CurrentControlAuthorizer {
    async fn subject(&self, credential: &Credential) -> Option<AuthenticatedSubject> {
        let context = self.context.upgrade()?;
        let authorizer = context.current().authorizer.clone()?;
        let subject = authorizer.subject(credential).await;
        Some(AuthenticatedSubject {
            principal: subject.identity?,
            claims: subject.claims,
        })
    }

    async fn authorize_platform_admin(&self, credential: &Credential) -> Option<String> {
        let context = self.context.upgrade()?;
        let authorizer = context.current().authorizer.clone()?;
        match authorizer.authorize_platform_admin(credential).await {
            tellurion_core::auth::PlatformAdminDecision::Allow { principal } => Some(principal),
            tellurion_core::auth::PlatformAdminDecision::Deny(_) => None,
        }
    }
}

pub(crate) struct ControlBrowserAuth {
    config: ControlBrowserAuthConfig,
    client_secret: Option<String>,
    sessions: Arc<dyn ControlSessionStore>,
    transport: Arc<dyn OidcTransport>,
    identity: Arc<dyn BrowserIdentityVerifier>,
    control_authorizer: Arc<dyn ControlCredentialAuthorizer>,
    endpoints: Mutex<Option<OidcEndpoints>>,
}

impl ControlBrowserAuth {
    pub(crate) fn new(
        config: ControlBrowserAuthConfig,
        client_secret: Option<String>,
        issuer_configs: impl IntoIterator<Item = OidcConfig>,
        context: &Arc<AppContext>,
    ) -> anyhow::Result<Arc<Self>> {
        let matching_issuers: Vec<_> = issuer_configs
            .into_iter()
            .filter(|issuer| issuer.issuer == config.issuer)
            .collect();
        let transport = Arc::new(ReqwestOidcTransport::new()?);
        Ok(Arc::new(Self {
            sessions: Arc::new(InMemoryControlSessionStore::new(config.max_sessions)),
            identity: Arc::new(TrustedBrowserIdentity {
                issuers: TrustedIssuerSet::new_for_browser(matching_issuers, &config.client_id),
            }),
            control_authorizer: Arc::new(CurrentControlAuthorizer {
                context: Arc::downgrade(context),
            }),
            config,
            client_secret,
            transport,
            endpoints: Mutex::new(None),
        }))
    }

    #[cfg(test)]
    pub(crate) fn new_with_dependencies(
        config: ControlBrowserAuthConfig,
        client_secret: Option<String>,
        sessions: Arc<dyn ControlSessionStore>,
        transport: Arc<dyn OidcTransport>,
        identity: Arc<dyn BrowserIdentityVerifier>,
        control_authorizer: Arc<dyn ControlCredentialAuthorizer>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            client_secret,
            sessions,
            transport,
            identity,
            control_authorizer,
            endpoints: Mutex::new(None),
        })
    }

    async fn endpoints(&self) -> Result<OidcEndpoints, ()> {
        let mut cached = self.endpoints.lock().await;
        if let Some(endpoints) = cached.as_ref() {
            return Ok(endpoints.clone());
        }
        let endpoints = self.transport.discover(&self.config.issuer).await?;
        *cached = Some(endpoints.clone());
        Ok(endpoints)
    }

    pub(crate) async fn resolve_request(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<ResolvedControlSession>, ResolveControlSessionError> {
        let bearer = crate::app::extract_credential(headers);
        let bearer_subject = match &bearer {
            Credential::None => None,
            Credential::Bearer(_) => self.control_authorizer.subject(&bearer).await,
        };
        let cookie = match session_cookie(headers) {
            Ok(Some(id)) => self
                .sessions
                .resolve(&id)
                .await
                .map_err(|_| ResolveControlSessionError)?
                .map(|session| (id, session)),
            Ok(None) | Err(()) => None,
        };
        let cookie_subject = if let Some((_, session)) = cookie.as_ref() {
            let credential = Credential::Bearer(session.access_token.clone());
            self.control_authorizer
                .subject(&credential)
                .await
                .filter(|subject| subject.principal == session.principal)
                .map(|subject| (credential, subject))
        } else {
            None
        };

        match (bearer_subject, cookie_subject) {
            (Some(bearer_subject), Some((credential, cookie_subject))) => {
                if bearer_subject.principal != cookie_subject.principal {
                    return Err(ResolveControlSessionError);
                }
                let (_, session) = cookie.expect("resolved cookie remains present");
                Ok(Some(ResolvedControlSession {
                    credential,
                    csrf: session.csrf_token,
                    principal: principal_name(&cookie_subject.principal),
                }))
            }
            (Some(subject), None) => Ok(Some(ResolvedControlSession {
                credential: bearer,
                csrf: String::new(),
                principal: principal_name(&subject.principal),
            })),
            (None, Some((credential, subject))) => {
                let (_, session) = cookie.expect("resolved cookie remains present");
                Ok(Some(ResolvedControlSession {
                    credential,
                    csrf: session.csrf_token,
                    principal: principal_name(&subject.principal),
                }))
            }
            (None, None) => Ok(None),
        }
    }

    pub(crate) fn cookie_mutation_is_valid(
        &self,
        headers: &HeaderMap,
        resolved: &ResolvedControlSession,
    ) -> bool {
        resolved.csrf.is_empty()
            || (exact_header(headers, &header::ORIGIN)
                == Some(self.config.public_origin.trim_end_matches('/'))
                && exact_header(headers, &axum::http::HeaderName::from_static(CSRF_HEADER))
                    .is_some_and(|presented| constant_time_eq(presented, &resolved.csrf)))
    }
}

pub(crate) struct ResolvedControlSession {
    pub credential: Credential,
    pub csrf: String,
    pub principal: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolveControlSessionError;

pub(crate) fn router<S>(auth: Arc<ControlBrowserAuth>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/_auth/control/login", get(login))
        .route("/_auth/control/callback", get(callback))
        .route("/_auth/control/session", get(session))
        .route("/_auth/control/logout", post(logout))
        .layer(Extension(auth))
        .layer(axum::middleware::from_fn(auth_responses_no_store))
}

async fn auth_responses_no_store(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    no_store(next.run(request).await)
}

pub(crate) async fn auth_paths_no_store(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let is_control_auth = request.uri().path().starts_with("/_auth/control/");
    let response = next.run(request).await;
    if is_control_auth {
        no_store(response)
    } else {
        response
    }
}

async fn login(
    Extension(auth): Extension<Arc<ControlBrowserAuth>>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let Some(return_to) = validated_return_to(raw_query.as_deref()) else {
        return auth_failure(StatusCode::BAD_REQUEST);
    };
    let Ok(endpoints) = auth.endpoints().await else {
        return auth_failure(StatusCode::BAD_GATEWAY);
    };
    let state = opaque_value();
    let browser_binding = opaque_value();
    let nonce = opaque_value();
    let verifier = opaque_value();
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let pending = PendingControlLogin {
        state: state.clone(),
        browser_binding: browser_binding.clone(),
        nonce: nonce.clone(),
        pkce_verifier: verifier,
        return_to,
        expires_at: Instant::now() + Duration::from_secs(auth.config.login_ttl_s),
    };
    if auth.sessions.begin_login(pending).await.is_err() {
        return auth_failure(StatusCode::SERVICE_UNAVAILABLE);
    }

    let mut redirect = endpoints.authorization;
    redirect
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &auth.config.client_id)
        .append_pair("redirect_uri", &auth.config.callback_url())
        .append_pair("scope", &auth.config.scopes.join(" "))
        .append_pair("state", &state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    let Ok(location) = HeaderValue::from_str(redirect.as_str()) else {
        return auth_failure(StatusCode::BAD_GATEWAY);
    };
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(header::LOCATION, location);
    let binding_cookie = HeaderValue::from_str(&login_binding_cookie(
        &browser_binding,
        auth.config.login_ttl_s,
    ))
    .expect("opaque login bindings produce valid cookie values");
    response
        .headers_mut()
        .insert(header::SET_COOKIE, binding_cookie);
    no_store(response)
}

async fn callback(
    Extension(auth): Extension<Arc<ControlBrowserAuth>>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
) -> Response {
    clear_login_binding_cookie(callback_inner(auth, raw_query, headers).await)
}

async fn callback_inner(
    auth: Arc<ControlBrowserAuth>,
    raw_query: Option<String>,
    headers: HeaderMap,
) -> Response {
    let Some(query) = callback_query(raw_query.as_deref()) else {
        return auth_failure(StatusCode::BAD_REQUEST);
    };
    let Some(state) = query.state.as_deref().filter(|state| !state.is_empty()) else {
        return auth_failure(StatusCode::BAD_REQUEST);
    };
    let pending = match auth.sessions.consume_login(state).await {
        Ok(Some(pending)) => pending,
        Ok(None) => return auth_failure(StatusCode::BAD_REQUEST),
        Err(_) => return auth_failure(StatusCode::SERVICE_UNAVAILABLE),
    };
    let presented_binding = control_login_cookie(&headers).ok().flatten();
    if !presented_binding
        .as_deref()
        .is_some_and(|binding| constant_time_eq(binding, &pending.browser_binding))
    {
        return auth_failure(StatusCode::BAD_REQUEST);
    }
    if query
        .issuer
        .as_deref()
        .is_some_and(|issuer| issuer != auth.config.issuer)
    {
        return auth_failure(StatusCode::BAD_REQUEST);
    }
    if query.error.is_some() {
        return auth_failure(StatusCode::BAD_REQUEST);
    }
    let Some(code) = query.code.filter(|code| !code.is_empty()) else {
        return auth_failure(StatusCode::BAD_REQUEST);
    };
    let endpoints = match auth.endpoints().await {
        Ok(endpoints) => endpoints,
        Err(()) => return auth_failure(StatusCode::BAD_GATEWAY),
    };
    let tokens = match auth
        .transport
        .exchange(
            &endpoints.token,
            TokenExchange {
                code,
                verifier: pending.pkce_verifier,
                redirect_uri: auth.config.callback_url(),
                client_id: auth.config.client_id.clone(),
                client_secret: auth.client_secret.clone(),
            },
        )
        .await
    {
        Ok(tokens) if !tokens.access_token.is_empty() && !tokens.id_token.is_empty() => tokens,
        Ok(_) | Err(()) => return auth_failure(StatusCode::BAD_GATEWAY),
    };
    let id_principal = match auth.identity.verify(&tokens.id_token, &pending.nonce).await {
        Ok(principal) => principal,
        Err(()) => return auth_failure(StatusCode::BAD_REQUEST),
    };
    let credential = Credential::Bearer(tokens.access_token.clone());
    let expected_principal = principal_name(&id_principal);
    if auth
        .control_authorizer
        .authorize_platform_admin(&credential)
        .await
        .as_deref()
        != Some(expected_principal.as_str())
    {
        return auth_failure(StatusCode::FORBIDDEN);
    }
    let session_ttl = tokens
        .expires_in_s
        .unwrap_or(auth.config.session_ttl_s)
        .min(auth.config.session_ttl_s);
    if session_ttl == 0 {
        return auth_failure(StatusCode::BAD_GATEWAY);
    }
    let session = ControlBrowserSession::new(
        id_principal,
        tokens.access_token,
        Instant::now() + Duration::from_secs(session_ttl),
    );
    let session_id = match auth.sessions.create(session).await {
        Ok(id) => id,
        Err(_) => return auth_failure(StatusCode::SERVICE_UNAVAILABLE),
    };
    let Ok(cookie) = HeaderValue::from_str(&set_cookie(&session_id)) else {
        return auth_failure(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let Ok(location) = HeaderValue::from_str(&pending.return_to) else {
        return auth_failure(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let mut response = StatusCode::SEE_OTHER.into_response();
    response.headers_mut().insert(header::LOCATION, location);
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    no_store(response)
}

async fn session(
    Extension(auth): Extension<Arc<ControlBrowserAuth>>,
    headers: HeaderMap,
) -> Response {
    let resolved = match session_cookie(&headers) {
        Ok(Some(id)) => match auth.sessions.resolve(&id).await {
            Ok(Some(session)) => {
                let credential = Credential::Bearer(session.access_token.clone());
                auth.control_authorizer
                    .subject(&credential)
                    .await
                    .filter(|subject| subject.principal == session.principal)
                    .map(|subject| SessionView {
                        authenticated: true,
                        principal: Some(principal_name(&subject.principal)),
                        csrf_token: Some(session.csrf_token),
                        expires_in_s: Some(
                            session
                                .expires_at
                                .saturating_duration_since(Instant::now())
                                .as_secs(),
                        ),
                    })
            }
            Ok(None) | Err(_) => None,
        },
        Ok(None) | Err(()) => None,
    }
    .unwrap_or(SessionView {
        authenticated: false,
        principal: None,
        csrf_token: None,
        expires_in_s: None,
    });
    no_store(Json(resolved).into_response())
}

async fn logout(
    Extension(auth): Extension<Arc<ControlBrowserAuth>>,
    headers: HeaderMap,
) -> Response {
    if exact_header(&headers, &header::ORIGIN)
        != Some(auth.config.public_origin.trim_end_matches('/'))
    {
        return auth_failure(StatusCode::FORBIDDEN);
    }
    let id = match session_cookie(&headers) {
        Ok(Some(id)) => id,
        Ok(None) | Err(()) => return auth_failure(StatusCode::UNAUTHORIZED),
    };
    let active = match auth.sessions.resolve(&id).await {
        Ok(Some(active)) => active,
        Ok(None) | Err(_) => return auth_failure(StatusCode::UNAUTHORIZED),
    };
    if !exact_header(&headers, &axum::http::HeaderName::from_static(CSRF_HEADER))
        .is_some_and(|presented| constant_time_eq(presented, &active.csrf_token))
    {
        return auth_failure(StatusCode::FORBIDDEN);
    }
    if auth.sessions.delete(&id).await.is_err() {
        return auth_failure(StatusCode::SERVICE_UNAVAILABLE);
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "tellurion_control_session=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0",
        ),
    );
    no_store(response)
}

#[derive(Serialize)]
struct SessionView {
    authenticated: bool,
    principal: Option<String>,
    csrf_token: Option<String>,
    expires_in_s: Option<u64>,
}

struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    issuer: Option<String>,
}

fn callback_query(raw_query: Option<&str>) -> Option<CallbackQuery> {
    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut error_description = None;
    let mut issuer = None;
    let mut session_state = None;
    for (key, value) in url::form_urlencoded::parse(raw_query?.as_bytes()) {
        if key.len() > MAX_CALLBACK_PARAMETER_BYTES || value.len() > MAX_CALLBACK_PARAMETER_BYTES {
            return None;
        }
        let target = match key.as_ref() {
            "code" => &mut code,
            "state" => &mut state,
            "error" => &mut error,
            "error_description" => &mut error_description,
            "iss" => &mut issuer,
            "session_state" => &mut session_state,
            _ => return None,
        };
        if target.replace(value.into_owned()).is_some() {
            return None;
        }
    }
    Some(CallbackQuery {
        code,
        state,
        error,
        issuer,
    })
}

fn validated_return_to(raw_query: Option<&str>) -> Option<String> {
    let raw_query = raw_query?;
    if raw_query.contains('&') || !raw_query.starts_with("return_to=") {
        return None;
    }
    let raw_value = raw_query.strip_prefix("return_to=")?;
    if raw_value.is_empty() || raw_value.contains('%') || raw_value.contains('+') {
        return None;
    }
    let value = url::form_urlencoded::parse(raw_query.as_bytes())
        .find_map(|(key, value)| (key == "return_to").then(|| value.into_owned()))?;
    if !value.starts_with("/ui/")
        || value.starts_with("//")
        || value.contains(['?', '#', '\\', '\0'])
        || value
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
    {
        return None;
    }
    Some(value)
}

fn session_cookie(headers: &HeaderMap) -> Result<Option<String>, ()> {
    let mut session_id = None;
    for cookie_header in headers.get_all(header::COOKIE) {
        let value = cookie_header.to_str().map_err(|_| ())?;
        for cookie in value.split(';') {
            let Some((name, value)) = cookie.trim().split_once('=') else {
                continue;
            };
            if name != CONTROL_SESSION_COOKIE {
                continue;
            }
            if session_id.is_some()
                || value.len() != 64
                || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(());
            }
            session_id = Some(value.to_string());
        }
    }
    Ok(session_id)
}

fn control_login_cookie(headers: &HeaderMap) -> Result<Option<String>, ()> {
    let mut binding = None;
    for cookie_header in headers.get_all(header::COOKIE) {
        let value = cookie_header.to_str().map_err(|_| ())?;
        for cookie in value.split(';') {
            let Some((name, value)) = cookie.trim().split_once('=') else {
                continue;
            };
            if name != CONTROL_LOGIN_COOKIE {
                continue;
            }
            if binding.is_some()
                || value.len() != 43
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(());
            }
            binding = Some(value.to_string());
        }
    }
    Ok(binding)
}

fn exact_header<'a>(headers: &'a HeaderMap, name: &axum::http::HeaderName) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn set_cookie(id: &str) -> String {
    format!("{CONTROL_SESSION_COOKIE}={id}; HttpOnly; Secure; SameSite=Lax; Path=/")
}

fn login_binding_cookie(binding: &str, ttl_s: u64) -> String {
    format!(
        "{CONTROL_LOGIN_COOKIE}={binding}; HttpOnly; Secure; SameSite=Lax; Path={CONTROL_CALLBACK_PATH}; Max-Age={ttl_s}"
    )
}

fn clear_login_binding_cookie(mut response: Response) -> Response {
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_static(CONTROL_LOGIN_CLEAR_COOKIE),
    );
    response
}

fn principal_name(principal: &PrincipalIdentity) -> String {
    format!("{}#{}", principal.issuer, principal.subject)
}

fn opaque_value() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn auth_failure(status: StatusCode) -> Response {
    no_store((status, "authentication failed").into_response())
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use tellurion_core::auth::Credential;
    use tellurion_core::AuthenticatedSubject;
    use tellurion_core::PrincipalIdentity;
    use tower::ServiceExt;

    use crate::control_session::{
        ControlSessionStore, InMemoryControlSessionStore, PendingControlLogin,
    };

    use super::*;

    const TEST_BROWSER_BINDING: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const TEST_ATTACKER_BINDING: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

    struct FakeTransport {
        endpoints: OidcEndpoints,
        token_result: StdMutex<Option<Result<OidcTokens, ()>>>,
        exchanges: StdMutex<Vec<TokenExchange>>,
        exchange_endpoints: StdMutex<Vec<Url>>,
    }

    #[async_trait]
    impl OidcTransport for FakeTransport {
        async fn discover(&self, _: &str) -> Result<OidcEndpoints, ()> {
            Ok(self.endpoints.clone())
        }

        async fn exchange(&self, endpoint: &Url, request: TokenExchange) -> Result<OidcTokens, ()> {
            self.exchange_endpoints
                .lock()
                .unwrap()
                .push(endpoint.clone());
            self.exchanges.lock().unwrap().push(request);
            self.token_result
                .lock()
                .unwrap()
                .take()
                .expect("one token result per callback")
        }
    }

    struct FakeIdentity {
        expected_id_token: &'static str,
        expected_nonce: &'static str,
        principal: PrincipalIdentity,
        reject: bool,
    }

    #[async_trait]
    impl BrowserIdentityVerifier for FakeIdentity {
        async fn verify(&self, id_token: &str, nonce: &str) -> Result<PrincipalIdentity, ()> {
            if self.reject || id_token != self.expected_id_token || nonce != self.expected_nonce {
                return Err(());
            }
            Ok(self.principal.clone())
        }
    }

    struct FakeControlAuthorizer {
        principal: PrincipalIdentity,
        sysadmin: bool,
    }

    #[async_trait]
    impl ControlCredentialAuthorizer for FakeControlAuthorizer {
        async fn subject(&self, credential: &Credential) -> Option<AuthenticatedSubject> {
            matches!(credential, Credential::Bearer(token) if token == "upstream-access").then(
                || AuthenticatedSubject {
                    principal: self.principal.clone(),
                    claims: HashMap::new(),
                },
            )
        }

        async fn authorize_platform_admin(&self, credential: &Credential) -> Option<String> {
            (self.sysadmin
                && matches!(credential, Credential::Bearer(token) if token == "upstream-access"))
            .then(|| format!("{}#{}", self.principal.issuer, self.principal.subject))
        }
    }

    fn config() -> ControlBrowserAuthConfig {
        ControlBrowserAuthConfig {
            issuer: "https://id.example.com".to_string(),
            client_id: "control-ui".to_string(),
            client_secret_env: Some("CONTROL_BROWSER_SECRET".to_string()),
            public_origin: "https://console.example.com".to_string(),
            scopes: vec![
                "openid".to_string(),
                "profile".to_string(),
                "email".to_string(),
            ],
            session_ttl_s: 3600,
            login_ttl_s: 300,
            max_sessions: 16,
        }
    }

    fn auth(store: Arc<InMemoryControlSessionStore>) -> Arc<ControlBrowserAuth> {
        auth_with(
            store,
            Ok(OidcTokens {
                access_token: "upstream-access".to_string(),
                id_token: "browser-id-token".to_string(),
                expires_in_s: Some(600),
            }),
            false,
            true,
        )
    }

    fn principal() -> PrincipalIdentity {
        PrincipalIdentity {
            issuer: "https://id.example.com".to_string(),
            subject: "operator-1".to_string(),
        }
    }

    fn auth_with(
        store: Arc<InMemoryControlSessionStore>,
        token_result: Result<OidcTokens, ()>,
        reject_identity: bool,
        sysadmin: bool,
    ) -> Arc<ControlBrowserAuth> {
        ControlBrowserAuth::new_with_dependencies(
            config(),
            Some("client-secret".to_string()),
            store,
            Arc::new(FakeTransport {
                endpoints: OidcEndpoints {
                    authorization: Url::parse("https://id.example.com/authorize").unwrap(),
                    token: Url::parse("https://id.example.com/token").unwrap(),
                },
                token_result: StdMutex::new(Some(token_result)),
                exchanges: StdMutex::new(Vec::new()),
                exchange_endpoints: StdMutex::new(Vec::new()),
            }),
            Arc::new(FakeIdentity {
                expected_id_token: "browser-id-token",
                expected_nonce: "login-nonce",
                principal: principal(),
                reject: reject_identity,
            }),
            Arc::new(FakeControlAuthorizer {
                principal: principal(),
                sysadmin,
            }),
        )
    }

    async fn seed_login(store: &InMemoryControlSessionStore, state: &str, lifetime: Duration) {
        store
            .begin_login(PendingControlLogin {
                state: state.to_string(),
                browser_binding: TEST_BROWSER_BINDING.to_string(),
                nonce: "login-nonce".to_string(),
                pkce_verifier: "server-only-verifier".to_string(),
                return_to: "/ui/control".to_string(),
                expires_at: tokio::time::Instant::now() + lifetime,
            })
            .await
            .unwrap();
    }

    async fn response_text(response: axum::response::Response) -> String {
        String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn login_redirect_contains_pkce_state_nonce_client_and_scopes_but_not_verifier() {
        let store = Arc::new(InMemoryControlSessionStore::new(16));
        let response = router::<()>(auth(Arc::clone(&store)))
            .oneshot(
                Request::builder()
                    .uri("/_auth/control/login?return_to=/ui/control")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let location = response.headers()[header::LOCATION].to_str().unwrap();
        let redirect = Url::parse(location).unwrap();
        let query: std::collections::HashMap<_, _> = redirect.query_pairs().collect();
        assert_eq!(
            redirect.as_str().split('?').next().unwrap(),
            "https://id.example.com/authorize"
        );
        assert_eq!(query.get("response_type").unwrap(), "code");
        assert_eq!(query.get("client_id").unwrap(), "control-ui");
        assert_eq!(
            query.get("redirect_uri").unwrap(),
            "https://console.example.com/_auth/control/callback"
        );
        assert_eq!(query.get("scope").unwrap(), "openid profile email");
        assert_eq!(query.get("code_challenge_method").unwrap(), "S256");
        assert!(!query["state"].is_empty());
        assert!(!query["nonce"].is_empty());
        assert!(!query["code_challenge"].is_empty());
        let binding_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(binding_cookie.starts_with("tellurion_control_login="));
        for attribute in [
            "HttpOnly",
            "Secure",
            "SameSite=Lax",
            "Path=/_auth/control/callback",
            "Max-Age=300",
        ] {
            assert!(binding_cookie.contains(attribute), "missing {attribute}");
        }
        for protocol_value in [
            query["state"].as_ref(),
            query["nonce"].as_ref(),
            query["code_challenge"].as_ref(),
        ] {
            assert!(!binding_cookie.contains(protocol_value));
        }
        let binding = binding_cookie
            .split(';')
            .next()
            .unwrap()
            .strip_prefix("tellurion_control_login=")
            .unwrap();

        let pending = store.consume_login(&query["state"]).await.unwrap().unwrap();
        assert_eq!(pending.browser_binding, binding);
        assert_eq!(pending.nonce, query["nonce"]);
        assert_eq!(pending.return_to, "/ui/control");
        assert!(!pending.pkce_verifier.is_empty());
        assert!(!location.contains(&pending.pkce_verifier));
        assert!(pending.expires_at > tokio::time::Instant::now() + Duration::from_secs(299));
    }

    #[tokio::test]
    async fn login_refuses_non_ui_and_ambiguous_return_targets_before_starting_a_login() {
        let rejected = [
            "return_to=//evil.example/ui/",
            "return_to=https://evil.example/ui/",
            "return_to=/other/ui/",
            "return_to=/ui/../_control/v1/platform/overview",
            "return_to=/ui/./control",
            "return_to=/ui/control/../../metrics",
            "return_to=%2Fui%2Fcontrol",
            "return_to=/ui/%2e%2e/_control/v1/platform/overview",
            "return_to=/ui/%2E%2E/_control/v1/platform/overview",
            "return_to=/ui/%2f..%2f_control/v1/platform/overview",
            "return_to=/ui/%5c..%5c_control/v1/platform/overview",
            "return_to=/ui/control%3Fnext=https://evil.example",
            "return_to=/ui/control?next=/other",
            "return_to=/ui/control%23fragment",
            "return_to=/ui/control&next=/ui/other",
        ];
        for query in rejected {
            let store = Arc::new(InMemoryControlSessionStore::new(16));
            let response = router::<()>(auth(Arc::clone(&store)))
                .oneshot(
                    Request::builder()
                        .uri(format!("/_auth/control/login?{query}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
    }

    #[test]
    fn accepted_return_targets_stay_under_ui_after_browser_url_resolution() {
        let origin = Url::parse("https://console.example.com").unwrap();

        for query in [
            "return_to=/ui/control",
            "return_to=/ui/control/tenants/acme",
        ] {
            let accepted = validated_return_to(Some(query)).expect("safe deep link");
            let resolved = origin.join(&accepted).unwrap();
            assert_eq!(resolved.origin(), origin.origin());
            assert!(resolved.path().starts_with("/ui/"), "{resolved}");
        }
    }

    #[tokio::test]
    async fn method_generated_auth_responses_are_also_no_store() {
        let store = Arc::new(InMemoryControlSessionStore::new(16));
        let response = router::<()>(auth(store))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/_auth/control/login?return_to=/ui/control")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }

    #[tokio::test]
    async fn callback_requires_the_initiating_browser_binding_consumes_state_and_clears_cookie() {
        for (case, presented_binding) in [("missing", None), ("wrong", Some(TEST_ATTACKER_BINDING))]
        {
            let state = format!("{case}-binding-state");
            let store = Arc::new(InMemoryControlSessionStore::new(16));
            seed_login(&store, &state, Duration::from_secs(30)).await;
            let mut request = Request::builder().uri(format!(
                "/_auth/control/callback?code=supplied-code&state={state}"
            ));
            if let Some(binding) = presented_binding {
                request =
                    request.header(header::COOKIE, format!("tellurion_control_login={binding}"));
            }
            let response = router::<()>(auth(Arc::clone(&store)))
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{case}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(
                response.headers()[header::SET_COOKIE],
                "tellurion_control_login=; HttpOnly; Secure; SameSite=Lax; Path=/_auth/control/callback; Max-Age=0"
            );
            assert!(store.consume_login(&state).await.unwrap().is_none());

            let replay = router::<()>(auth(Arc::clone(&store)))
                .oneshot(
                    Request::builder()
                        .uri(format!(
                            "/_auth/control/callback?code=replay-code&state={state}"
                        ))
                        .header(
                            header::COOKIE,
                            format!("tellurion_control_login={TEST_BROWSER_BINDING}"),
                        )
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
            assert_eq!(
                replay.headers()[header::SET_COOKIE],
                "tellurion_control_login=; HttpOnly; Secure; SameSite=Lax; Path=/_auth/control/callback; Max-Age=0"
            );
        }
    }

    #[tokio::test]
    async fn callback_accepts_bounded_interoperability_parameters_and_enforces_exact_issuer() {
        let store = Arc::new(InMemoryControlSessionStore::new(16));
        seed_login(&store, "issuer-state", Duration::from_secs(30)).await;
        let response = router::<()>(auth(Arc::clone(&store)))
            .oneshot(
                Request::builder()
                    .uri("/_auth/control/callback?code=authorization-code&state=issuer-state&iss=https%3A%2F%2Fid.example.com&session_state=provider-session")
                    .header(
                        header::COOKIE,
                        format!("{CONTROL_LOGIN_COOKIE}={TEST_BROWSER_BINDING}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);

        seed_login(&store, "wrong-issuer-state", Duration::from_secs(30)).await;
        let response = router::<()>(auth(Arc::clone(&store)))
            .oneshot(
                Request::builder()
                    .uri("/_auth/control/callback?code=authorization-code&state=wrong-issuer-state&iss=https%3A%2F%2Fother.example.com")
                    .header(
                        header::COOKIE,
                        format!("{CONTROL_LOGIN_COOKIE}={TEST_BROWSER_BINDING}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(store
            .consume_login("wrong-issuer-state")
            .await
            .unwrap()
            .is_none());

        let oversized = "x".repeat(2049);
        for query in [
            "code=a&state=b&iss=one&iss=two".to_string(),
            "code=a&state=b&session_state=one&session_state=two".to_string(),
            format!("code=a&state=b&session_state={oversized}"),
        ] {
            assert!(callback_query(Some(&query)).is_none());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn callback_failure_matrix_is_generic_one_use_and_secret_free() {
        struct Case {
            name: &'static str,
            query: &'static str,
            seed: bool,
            lifetime: Duration,
            tokens: Result<OidcTokens, ()>,
            reject_identity: bool,
            sysadmin: bool,
        }

        let cases = [
            Case {
                name: "missing state",
                query: "code=supplied-code",
                seed: false,
                lifetime: Duration::from_secs(30),
                tokens: Err(()),
                reject_identity: false,
                sysadmin: true,
            },
            Case {
                name: "unknown state",
                query: "code=supplied-code&state=unknown-state",
                seed: false,
                lifetime: Duration::from_secs(30),
                tokens: Err(()),
                reject_identity: false,
                sysadmin: true,
            },
            Case {
                name: "expired state",
                query: "code=supplied-code&state=callback-state",
                seed: true,
                lifetime: Duration::from_secs(1),
                tokens: Err(()),
                reject_identity: false,
                sysadmin: true,
            },
            Case {
                name: "provider error",
                query: "error=access_denied&error_description=supplied-description&state=callback-state",
                seed: true,
                lifetime: Duration::from_secs(30),
                tokens: Err(()),
                reject_identity: false,
                sysadmin: true,
            },
            Case {
                name: "missing code",
                query: "state=callback-state",
                seed: true,
                lifetime: Duration::from_secs(30),
                tokens: Err(()),
                reject_identity: false,
                sysadmin: true,
            },
            Case {
                name: "token endpoint error",
                query: "code=supplied-code&state=callback-state",
                seed: true,
                lifetime: Duration::from_secs(30),
                tokens: Err(()),
                reject_identity: false,
                sysadmin: true,
            },
            Case {
                name: "missing access token",
                query: "code=supplied-code&state=callback-state",
                seed: true,
                lifetime: Duration::from_secs(30),
                tokens: Ok(OidcTokens {
                    access_token: String::new(),
                    id_token: "browser-id-token".to_string(),
                    expires_in_s: Some(600),
                }),
                reject_identity: false,
                sysadmin: true,
            },
            Case {
                name: "missing id token",
                query: "code=supplied-code&state=callback-state",
                seed: true,
                lifetime: Duration::from_secs(30),
                tokens: Ok(OidcTokens {
                    access_token: "upstream-access".to_string(),
                    id_token: String::new(),
                    expires_in_s: Some(600),
                }),
                reject_identity: false,
                sysadmin: true,
            },
            Case {
                name: "invalid token",
                query: "code=supplied-code&state=callback-state",
                seed: true,
                lifetime: Duration::from_secs(30),
                tokens: Ok(OidcTokens {
                    access_token: "upstream-access".to_string(),
                    id_token: "browser-id-token".to_string(),
                    expires_in_s: Some(600),
                }),
                reject_identity: true,
                sysadmin: true,
            },
            Case {
                name: "authenticated non-sysadmin",
                query: "code=supplied-code&state=callback-state",
                seed: true,
                lifetime: Duration::from_secs(30),
                tokens: Ok(OidcTokens {
                    access_token: "upstream-access".to_string(),
                    id_token: "browser-id-token".to_string(),
                    expires_in_s: Some(600),
                }),
                reject_identity: false,
                sysadmin: false,
            },
        ];

        for case in cases {
            let store = Arc::new(InMemoryControlSessionStore::new(16));
            if case.seed {
                seed_login(&store, "callback-state", case.lifetime).await;
            }
            if case.name == "expired state" {
                tokio::time::advance(Duration::from_secs(2)).await;
            }
            let response = router::<()>(auth_with(
                Arc::clone(&store),
                case.tokens,
                case.reject_identity,
                case.sysadmin,
            ))
            .oneshot(
                Request::builder()
                    .uri(format!("/_auth/control/callback?{}", case.query))
                    .header(
                        header::COOKIE,
                        format!("{CONTROL_LOGIN_COOKIE}={TEST_BROWSER_BINDING}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
            assert!(response.status().is_client_error() || response.status().is_server_error());
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert!(
                response
                    .headers()
                    .get_all(header::SET_COOKIE)
                    .iter()
                    .filter_map(|value| value.to_str().ok())
                    .any(|value| value == CONTROL_LOGIN_CLEAR_COOKIE),
                "{} did not clear the login binding cookie",
                case.name
            );
            let body = response_text(response).await;
            assert_eq!(body, "authentication failed", "{}", case.name);
            for supplied in [
                "supplied-code",
                "unknown-state",
                "callback-state",
                "access_denied",
                "supplied-description",
                "upstream-access",
                "browser-id-token",
                "server-only-verifier",
                "client-secret",
            ] {
                assert!(
                    !body.contains(supplied),
                    "{} leaked in {}",
                    supplied,
                    case.name
                );
            }
        }

        let store = Arc::new(InMemoryControlSessionStore::new(16));
        seed_login(&store, "one-use-state", Duration::from_secs(30)).await;
        let first = router::<()>(auth_with(Arc::clone(&store), Err(()), false, true))
            .oneshot(
                Request::builder()
                    .uri("/_auth/control/callback?code=one&state=one-use-state")
                    .header(
                        header::COOKIE,
                        format!("{CONTROL_LOGIN_COOKIE}={TEST_BROWSER_BINDING}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(first.status().is_server_error());
        let replay = router::<()>(auth_with(Arc::clone(&store), Err(()), false, true))
            .oneshot(
                Request::builder()
                    .uri("/_auth/control/callback?code=two&state=one-use-state")
                    .header(
                        header::COOKIE,
                        format!("{CONTROL_LOGIN_COOKIE}={TEST_BROWSER_BINDING}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response_text(replay).await, "authentication failed");
    }

    #[tokio::test]
    async fn callback_rejects_an_id_token_bound_to_a_different_nonce() {
        let store = Arc::new(InMemoryControlSessionStore::new(16));
        store
            .begin_login(PendingControlLogin {
                state: "nonce-state".to_string(),
                browser_binding: TEST_BROWSER_BINDING.to_string(),
                nonce: "different-login-nonce".to_string(),
                pkce_verifier: "server-only-verifier".to_string(),
                return_to: "/ui/control".to_string(),
                expires_at: tokio::time::Instant::now() + Duration::from_secs(30),
            })
            .await
            .unwrap();
        let response = router::<()>(auth(Arc::clone(&store)))
            .oneshot(
                Request::builder()
                    .uri("/_auth/control/callback?code=nonce-code&state=nonce-state")
                    .header(
                        header::COOKIE,
                        format!("{CONTROL_LOGIN_COOKIE}={TEST_BROWSER_BINDING}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response_text(response).await, "authentication failed");
    }

    #[tokio::test]
    async fn callback_uses_stored_verifier_creates_session_and_sets_only_opaque_cookie() {
        let store = Arc::new(InMemoryControlSessionStore::new(16));
        seed_login(&store, "callback-state", Duration::from_secs(30)).await;
        let transport = Arc::new(FakeTransport {
            endpoints: OidcEndpoints {
                authorization: Url::parse("https://id.example.com/authorize").unwrap(),
                token: Url::parse("https://id.example.com/token").unwrap(),
            },
            token_result: StdMutex::new(Some(Ok(OidcTokens {
                access_token: "upstream-access".to_string(),
                id_token: "browser-id-token".to_string(),
                expires_in_s: Some(600),
            }))),
            exchanges: StdMutex::new(Vec::new()),
            exchange_endpoints: StdMutex::new(Vec::new()),
        });
        let auth = ControlBrowserAuth::new_with_dependencies(
            config(),
            Some("client-secret".to_string()),
            Arc::clone(&store) as Arc<dyn ControlSessionStore>,
            Arc::clone(&transport) as Arc<dyn OidcTransport>,
            Arc::new(FakeIdentity {
                expected_id_token: "browser-id-token",
                expected_nonce: "login-nonce",
                principal: principal(),
                reject: false,
            }),
            Arc::new(FakeControlAuthorizer {
                principal: principal(),
                sysadmin: true,
            }),
        );

        let response = router::<()>(auth)
            .oneshot(
                Request::builder()
                    .uri("/_auth/control/callback?code=authorization-code&state=callback-state")
                    .header(
                        header::COOKIE,
                        format!("{CONTROL_LOGIN_COOKIE}={TEST_BROWSER_BINDING}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::LOCATION], "/ui/control");
        let cookies = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();
        assert!(cookies.contains(&CONTROL_LOGIN_CLEAR_COOKIE));
        let cookie = cookies
            .iter()
            .find(|value| value.starts_with("tellurion_control_session="))
            .expect("session cookie");
        assert!(cookie.starts_with("tellurion_control_session="));
        for attribute in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/"] {
            assert!(cookie.contains(attribute), "missing {attribute}");
        }
        for secret in [
            "authorization-code",
            "callback-state",
            "server-only-verifier",
            "upstream-access",
            "browser-id-token",
            "client-secret",
            "login-nonce",
        ] {
            assert!(!cookie.contains(secret), "cookie leaked {secret}");
        }
        let session_id = cookie
            .split(';')
            .next()
            .unwrap()
            .strip_prefix("tellurion_control_session=")
            .unwrap();
        let session = store.resolve(session_id).await.unwrap().unwrap();
        assert_eq!(session.principal, principal());
        assert_eq!(session.access_token, "upstream-access");
        assert_eq!(session.csrf_token.len(), 64);
        assert!(session.expires_at > tokio::time::Instant::now() + Duration::from_secs(599));
        assert!(session.expires_at <= tokio::time::Instant::now() + Duration::from_secs(600));

        let exchanges = transport.exchanges.lock().unwrap();
        assert_eq!(exchanges.len(), 1);
        assert_eq!(exchanges[0].code, "authorization-code");
        assert_eq!(exchanges[0].verifier, "server-only-verifier");
        assert_eq!(exchanges[0].client_id, "control-ui");
        assert_eq!(exchanges[0].client_secret.as_deref(), Some("client-secret"));
        assert_eq!(
            exchanges[0].redirect_uri,
            "https://console.example.com/_auth/control/callback"
        );
        assert_eq!(
            transport.exchange_endpoints.lock().unwrap().as_slice(),
            [Url::parse("https://id.example.com/token").unwrap()]
        );
    }

    #[tokio::test]
    async fn session_response_exposes_only_browser_metadata_never_upstream_tokens() {
        let store = Arc::new(InMemoryControlSessionStore::new(16));
        let session_id = store
            .create(crate::control_session::ControlBrowserSession::new(
                principal(),
                "upstream-access".to_string(),
                tokio::time::Instant::now() + Duration::from_secs(600),
            ))
            .await
            .unwrap();
        let response = router::<()>(auth(Arc::clone(&store)))
            .oneshot(
                Request::builder()
                    .uri("/_auth/control/session")
                    .header(
                        header::COOKIE,
                        format!("tellurion_control_session={session_id}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let body = response_text(response).await;
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let keys: std::collections::BTreeSet<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["authenticated", "csrf_token", "expires_in_s", "principal"]
                .into_iter()
                .collect()
        );
        assert_eq!(value["authenticated"], true);
        assert_eq!(value["principal"], "https://id.example.com#operator-1");
        assert_eq!(value["csrf_token"].as_str().unwrap().len(), 64);
        assert!(value["expires_in_s"].as_u64().unwrap() <= 600);
        for secret in ["upstream-access", "browser-id-token", &session_id] {
            assert!(!body.contains(secret));
        }
    }

    #[tokio::test]
    async fn logout_requires_exact_origin_and_csrf_then_clears_identical_cookie_attributes() {
        let store = Arc::new(InMemoryControlSessionStore::new(16));
        let active = crate::control_session::ControlBrowserSession::new(
            principal(),
            "upstream-access".to_string(),
            tokio::time::Instant::now() + Duration::from_secs(600),
        );
        let csrf = active.csrf_token.clone();
        let session_id = store.create(active).await.unwrap();
        let cookie = format!("tellurion_control_session={session_id}");

        for (origin, presented_csrf) in [
            (None, Some(csrf.as_str())),
            (Some("https://foreign.example"), Some(csrf.as_str())),
            (Some("https://console.example.com/"), Some(csrf.as_str())),
            (Some("https://console.example.com"), None),
            (Some("https://console.example.com"), Some("wrong-csrf")),
        ] {
            let mut request = Request::builder()
                .method("POST")
                .uri("/_auth/control/logout")
                .header(header::COOKIE, &cookie);
            if let Some(origin) = origin {
                request = request.header(header::ORIGIN, origin);
            }
            if let Some(presented_csrf) = presented_csrf {
                request = request.header("x-tellurion-csrf", presented_csrf);
            }
            let response = router::<()>(auth(Arc::clone(&store)))
                .oneshot(request.body(Body::from("ignored-body")).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert!(store.resolve(&session_id).await.unwrap().is_some());
        }

        let response = router::<()>(auth(Arc::clone(&store)))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/_auth/control/logout")
                    .header(header::COOKIE, &cookie)
                    .header(header::ORIGIN, "https://console.example.com")
                    .header("x-tellurion-csrf", &csrf)
                    .body(Body::from("ignored-body"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let cleared = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert_eq!(
            cleared,
            "tellurion_control_session=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0"
        );
        assert!(store.resolve(&session_id).await.unwrap().is_none());
    }

    #[test]
    fn discovery_metadata_is_bounded_and_accepts_only_https_or_exact_loopback_http() {
        let valid = br#"{
            "issuer":"https://id.example.com",
            "authorization_endpoint":"https://id.example.com/authorize?prompt=login",
            "token_endpoint":"http://127.0.0.1:8080/token?tenant=example"
        }"#;
        let endpoints = parse_discovery_document(valid, "https://id.example.com").unwrap();
        assert_eq!(
            endpoints.authorization,
            Url::parse("https://id.example.com/authorize?prompt=login").unwrap()
        );
        assert_eq!(
            endpoints.token,
            Url::parse("http://127.0.0.1:8080/token?tenant=example").unwrap()
        );

        for rejected in [
            br#"{"authorization_endpoint":"https://id.example.com/authorize","token_endpoint":"https://id.example.com/token"}"#.as_slice(),
            br#"{"issuer":"https://other.example.com","authorization_endpoint":"https://id.example.com/authorize","token_endpoint":"https://id.example.com/token"}"#.as_slice(),
            br#"{"issuer":"https://id.example.com","authorization_endpoint":"http://id.example.com/authorize","token_endpoint":"https://id.example.com/token"}"#.as_slice(),
            br#"{"issuer":"https://id.example.com","authorization_endpoint":"https://user@id.example.com/authorize","token_endpoint":"https://id.example.com/token"}"#.as_slice(),
            br#"{"issuer":"https://id.example.com","authorization_endpoint":"https://id.example.com/authorize#fragment","token_endpoint":"https://id.example.com/token"}"#.as_slice(),
            br#"{"issuer":"https://id.example.com","authorization_endpoint":"https://id.example.com/authorize","token_endpoint":"http://192.0.2.1/token"}"#.as_slice(),
            br#"{"issuer":"https://id.example.com","authorization_endpoint":"https://id.example.com/authorize","token_endpoint":"file:///tmp/token"}"#.as_slice(),
            br#"{"issuer":"https://id.example.com","authorization_endpoint":"https://id.example.com/authorize","token_endpoint":"https://secret@id.example.com/token"}"#.as_slice(),
            br#"{"issuer":"https://id.example.com","authorization_endpoint":"https://id.example.com/authorize","token_endpoint":"https://id.example.com/token#fragment"}"#.as_slice(),
            br#"{"issuer":"https://id.example.com","authorization_endpoint":"https://id.example.com/authorize"}"#.as_slice(),
            b"not-json".as_slice(),
        ] {
            assert!(parse_discovery_document(rejected, "https://id.example.com").is_err());
        }

        let oversized = vec![b' '; MAX_OIDC_RESPONSE_BYTES + 1];
        assert!(parse_discovery_document(&oversized, "https://id.example.com").is_err());
    }
}
