//! Cancellation without connection poisoning. The query runs on a background
//! task that owns the pooled connection; if the caller's future is dropped
//! before that task finishes (client disconnect, tower timeout, load shed),
//! the in-flight statement is cancelled server-side via `CancelToken` and the
//! background task is left to drain. The connection is only ever returned to
//! the pool once the query actually completes (cancelled or not), so it is
//! never force-closed and never poisoned.

use std::future::Future;

use deadpool_postgres::Client;
use tokio_postgres::NoTls;

use crate::error::{PostgisError, Result};

struct CancelGuard {
    token: Option<tokio_postgres::CancelToken>,
}

impl CancelGuard {
    fn disarm(&mut self) {
        self.token = None;
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            tokio::spawn(async move {
                let _ = token.cancel_query(NoTls).await;
            });
        }
    }
}

/// Runs `query(client)` on a background task, holding `client` for the life
/// of that task. Dropping the returned future before it resolves fires a
/// best-effort cancel and detaches from the still-running task; the pool
/// still gets its connection back when that task eventually finishes.
pub(crate) async fn run_cancellable<T, F, Fut>(client: Client, query: F) -> Result<T>
where
    F: FnOnce(Client) -> Fut + Send + 'static,
    Fut: Future<Output = std::result::Result<T, tokio_postgres::Error>> + Send + 'static,
    T: Send + 'static,
{
    let mut guard = CancelGuard {
        token: Some(client.cancel_token()),
    };
    let handle = tokio::spawn(async move { query(client).await });

    let joined = handle.await;
    guard.disarm();

    match joined {
        Ok(result) => result.map_err(PostgisError::from),
        Err(join_err) => Err(PostgisError::from(join_err)),
    }
}
