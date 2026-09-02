use std::collections::HashMap;
use std::fmt;
use std::fmt::Write as _;

use async_trait::async_trait;
use rand::Rng;
use tellurion_core::PrincipalIdentity;
use tokio::sync::Mutex;
use tokio::time::Instant;

#[derive(Clone, PartialEq, Eq)]
pub struct PendingControlLogin {
    pub state: String,
    pub browser_binding: String,
    pub nonce: String,
    pub pkce_verifier: String,
    pub return_to: String,
    pub expires_at: Instant,
}

impl fmt::Debug for PendingControlLogin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingControlLogin")
            .field("state", &"[redacted]")
            .field("browser_binding", &"[redacted]")
            .field("nonce", &"[redacted]")
            .field("pkce_verifier", &"[redacted]")
            .field("return_to", &self.return_to)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ControlBrowserSession {
    pub principal: PrincipalIdentity,
    pub access_token: String,
    pub csrf_token: String,
    pub expires_at: Instant,
}

impl ControlBrowserSession {
    pub fn new(principal: PrincipalIdentity, access_token: String, expires_at: Instant) -> Self {
        Self {
            principal,
            access_token,
            csrf_token: opaque_id(),
            expires_at,
        }
    }
}

impl fmt::Debug for ControlBrowserSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControlBrowserSession")
            .field("principal", &self.principal)
            .field("access_token", &"[redacted]")
            .field("csrf_token", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlSessionError {
    Capacity,
}

impl fmt::Display for ControlSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capacity => formatter.write_str("control session capacity reached"),
        }
    }
}

impl std::error::Error for ControlSessionError {}

#[async_trait]
pub trait ControlSessionStore: Send + Sync {
    async fn begin_login(&self, login: PendingControlLogin) -> Result<(), ControlSessionError>;
    async fn consume_login(
        &self,
        state: &str,
    ) -> Result<Option<PendingControlLogin>, ControlSessionError>;
    async fn create(&self, session: ControlBrowserSession) -> Result<String, ControlSessionError>;
    async fn resolve(&self, id: &str)
        -> Result<Option<ControlBrowserSession>, ControlSessionError>;
    async fn delete(&self, id: &str) -> Result<(), ControlSessionError>;
}

pub struct InMemoryControlSessionStore {
    capacity: usize,
    pending_logins: Mutex<HashMap<String, PendingControlLogin>>,
    sessions: Mutex<HashMap<String, ControlBrowserSession>>,
}

impl InMemoryControlSessionStore {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            pending_logins: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
        }
    }
}

fn prune_expired<T>(values: &mut HashMap<String, T>, expires_at: impl Fn(&T) -> Instant) {
    let now = Instant::now();
    values.retain(|_, value| expires_at(value) > now);
}

fn opaque_id() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

#[async_trait]
impl ControlSessionStore for InMemoryControlSessionStore {
    async fn begin_login(&self, login: PendingControlLogin) -> Result<(), ControlSessionError> {
        let mut pending = self.pending_logins.lock().await;
        prune_expired(&mut pending, |value| value.expires_at);
        if !pending.contains_key(&login.state) && pending.len() >= self.capacity {
            return Err(ControlSessionError::Capacity);
        }
        pending.insert(login.state.clone(), login);
        Ok(())
    }

    async fn consume_login(
        &self,
        state: &str,
    ) -> Result<Option<PendingControlLogin>, ControlSessionError> {
        let mut pending = self.pending_logins.lock().await;
        prune_expired(&mut pending, |value| value.expires_at);
        Ok(pending.remove(state))
    }

    async fn create(&self, session: ControlBrowserSession) -> Result<String, ControlSessionError> {
        let mut sessions = self.sessions.lock().await;
        prune_expired(&mut sessions, |value| value.expires_at);
        if sessions.len() >= self.capacity {
            return Err(ControlSessionError::Capacity);
        }
        let mut id = opaque_id();
        while sessions.contains_key(&id) {
            id = opaque_id();
        }
        sessions.insert(id.clone(), session);
        Ok(id)
    }

    async fn resolve(
        &self,
        id: &str,
    ) -> Result<Option<ControlBrowserSession>, ControlSessionError> {
        let mut sessions = self.sessions.lock().await;
        prune_expired(&mut sessions, |value| value.expires_at);
        Ok(sessions.get(id).cloned())
    }

    async fn delete(&self, id: &str) -> Result<(), ControlSessionError> {
        let mut sessions = self.sessions.lock().await;
        prune_expired(&mut sessions, |value| value.expires_at);
        sessions.remove(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tellurion_core::PrincipalIdentity;

    use super::*;

    fn login(state: &str, lifetime: Duration) -> PendingControlLogin {
        PendingControlLogin {
            state: state.to_string(),
            browser_binding: "browser-binding-secret".to_string(),
            nonce: "login-nonce-secret".to_string(),
            pkce_verifier: "pkce-verifier-secret".to_string(),
            return_to: "/ui/control".to_string(),
            expires_at: tokio::time::Instant::now() + lifetime,
        }
    }

    fn session(lifetime: Duration) -> ControlBrowserSession {
        ControlBrowserSession::new(
            PrincipalIdentity {
                issuer: "https://id.example.com".to_string(),
                subject: "operator-1".to_string(),
            },
            "upstream-access-token-secret".to_string(),
            tokio::time::Instant::now() + lifetime,
        )
    }

    #[tokio::test(start_paused = true)]
    async fn pending_login_state_is_one_use_and_expired_values_disappear() {
        let store = InMemoryControlSessionStore::new(2);
        store
            .begin_login(login("one-use-state", Duration::from_secs(30)))
            .await
            .unwrap();

        assert!(store
            .consume_login("one-use-state")
            .await
            .unwrap()
            .is_some());
        assert!(store
            .consume_login("one-use-state")
            .await
            .unwrap()
            .is_none());

        store
            .begin_login(login("expired-state", Duration::from_secs(1)))
            .await
            .unwrap();
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(store
            .consume_login("expired-state")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn each_map_enforces_capacity_after_pruning_expired_values() {
        let store = InMemoryControlSessionStore::new(1);
        store
            .begin_login(login("first", Duration::from_secs(30)))
            .await
            .unwrap();
        assert_eq!(
            store
                .begin_login(login("second", Duration::from_secs(30)))
                .await,
            Err(ControlSessionError::Capacity)
        );

        let first_id = store.create(session(Duration::from_secs(1))).await.unwrap();
        assert_eq!(
            store.create(session(Duration::from_secs(30))).await,
            Err(ControlSessionError::Capacity)
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(store.resolve(&first_id).await.unwrap().is_none());
        assert!(store.create(session(Duration::from_secs(30))).await.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn sessions_expire_and_deletion_is_idempotent() {
        let store = InMemoryControlSessionStore::new(2);
        let id = store
            .create(session(Duration::from_secs(30)))
            .await
            .unwrap();
        assert_eq!(id.len(), 64, "a session id represents 256 bits");
        assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(store.resolve(&id).await.unwrap().is_some());

        store.delete(&id).await.unwrap();
        store.delete(&id).await.unwrap();
        assert!(store.resolve(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn csrf_token_is_high_entropy_session_bound_and_redacted() {
        let store = InMemoryControlSessionStore::new(1);
        let active = ControlBrowserSession::new(
            PrincipalIdentity {
                issuer: "https://id.example.com".to_string(),
                subject: "operator-1".to_string(),
            },
            "upstream-access-token-secret".to_string(),
            tokio::time::Instant::now() + Duration::from_secs(30),
        );
        let csrf_token = active.csrf_token.clone();
        assert_eq!(csrf_token.len(), 64, "CSRF token represents 256 bits");

        let id = store.create(active).await.unwrap();
        let resolved = store.resolve(&id).await.unwrap().unwrap();
        assert_eq!(resolved.csrf_token, csrf_token);
        assert!(!format!("{resolved:?}").contains(&csrf_token));
    }

    #[tokio::test]
    async fn debug_and_error_output_never_contains_stored_secrets() {
        let pending = login("state-secret", Duration::from_secs(30));
        let active = session(Duration::from_secs(30));
        for rendered in [
            format!("{pending:?}"),
            format!("{active:?}"),
            ControlSessionError::Capacity.to_string(),
            format!("{:?}", ControlSessionError::Capacity),
        ] {
            for secret in [
                "state-secret",
                "browser-binding-secret",
                "login-nonce-secret",
                "pkce-verifier-secret",
                "upstream-access-token-secret",
            ] {
                assert!(!rendered.contains(secret), "secret leaked in output");
            }
        }
    }
}
