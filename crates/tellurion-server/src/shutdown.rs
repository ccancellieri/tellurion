//! Signal handling and a bounded graceful drain: SIGINT/SIGTERM stop new
//! connections being accepted and let in-flight requests finish, but never
//! for more than the configured drain timeout — an operator killing the
//! process should not be able to wait forever on a stuck request.

use std::future::Future;
use std::time::Duration;

use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};

use crate::readiness::Readiness;

async fn wait_for_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install SIGINT handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT, starting graceful shutdown"),
        _ = terminate => tracing::info!("received SIGTERM, starting graceful shutdown"),
    }
}

/// Watches for SIGINT/SIGTERM and reports it on the returned channel, which
/// stays at `false` until the signal arrives. `main.rs` reads this once at
/// boot and shares the receiver between the HTTP server and background
/// tasks that need the same shutdown edge.
pub fn watch_signal() -> watch::Receiver<bool> {
    let (tx, rx) = watch::channel(false);
    tokio::spawn(async move {
        wait_for_signal().await;
        let _ = tx.send(true);
    });
    rx
}

/// Waits for a shared shutdown receiver without missing an edge delivered
/// before this adapter starts polling. A closed sender is also shutdown:
/// otherwise a failed signal-owner task could leave a waiter blocked forever.
pub async fn wait_for_shutdown(mut rx: watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

async fn run_until_shutdown_with<S, F, E, N>(
    serve: S,
    readiness: Readiness,
    drain_timeout: Duration,
    shutdown: F,
    notify_server: N,
) -> anyhow::Result<()>
where
    S: Future<Output = Result<(), E>>,
    F: Future<Output = ()>,
    E: std::error::Error + Send + Sync + 'static,
    N: FnOnce(),
{
    tokio::pin!(serve);
    tokio::pin!(shutdown);

    tokio::select! {
        result = &mut serve => return result.map_err(anyhow::Error::new),
        () = &mut shutdown => {}
    }

    let deadline_at = tokio::time::Instant::now() + drain_timeout;
    readiness.begin_draining();
    notify_server();
    let deadline = tokio::time::sleep_until(deadline_at);
    tokio::pin!(deadline);

    tokio::select! {
        result = &mut serve => {
            tracing::info!(
                event = "shutdown_drain",
                outcome = "completed",
                "graceful shutdown completed"
            );
            result.map_err(anyhow::Error::new)
        }
        () = &mut deadline => {
            tracing::warn!(
                event = "shutdown_drain",
                outcome = "deadline",
                timeout_ms = drain_timeout.as_millis(),
                "graceful shutdown deadline elapsed; forcing exit"
            );
            Ok(())
        }
    }
}

#[cfg(test)]
async fn run_until_shutdown<S, F, E>(
    serve: S,
    readiness: Readiness,
    drain_timeout: Duration,
    shutdown: F,
) -> anyhow::Result<()>
where
    S: Future<Output = Result<(), E>>,
    F: Future<Output = ()>,
    E: std::error::Error + Send + Sync + 'static,
{
    run_until_shutdown_with(serve, readiness, drain_timeout, shutdown, || {}).await
}

/// Serves `app` until `shutdown` resolves. At that edge readiness becomes
/// terminally draining before Axum stops accepting work, and the configured
/// deadline begins while already-admitted requests finish.
pub async fn serve_until<F>(
    listener: TcpListener,
    app: Router,
    readiness: Readiness,
    drain_timeout: Duration,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()>,
{
    let (notify_tx, notify_rx) = oneshot::channel();
    let serve = async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = notify_rx.await;
            })
            .await
    };
    run_until_shutdown_with(serve, readiness, drain_timeout, shutdown, move || {
        let _ = notify_tx.send(());
    })
    .await
}

/// Waits for every background task under one deadline that starts at the
/// shared shutdown edge. Any task still running at the deadline is aborted
/// so the process does not detach work while reporting a clean exit.
pub async fn supervise_tasks(
    mut tasks: Vec<tokio::task::JoinHandle<()>>,
    shutdown_rx: watch::Receiver<bool>,
    drain_timeout: Duration,
) {
    wait_for_shutdown(shutdown_rx).await;
    let completion = async {
        for task in &mut tasks {
            if let Err(error) = task.await {
                tracing::error!(%error, "background task exited unsuccessfully during shutdown");
            }
        }
    };

    if tokio::time::timeout(drain_timeout, completion)
        .await
        .is_err()
    {
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        tracing::warn!(
            timeout_ms = drain_timeout.as_millis(),
            "background tasks did not stop within the shared shutdown deadline"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::future;
    use std::io;
    use std::sync::Arc;
    use std::time::Duration;

    use axum::extract::State;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::{oneshot, Notify};

    use super::*;
    use crate::readiness::{Readiness, ReadinessStatus};

    #[derive(Clone)]
    struct RequestGate {
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    async fn gated_request(State(gate): State<RequestGate>) -> &'static str {
        gate.entered.notify_one();
        gate.release.notified().await;
        "completed"
    }

    async fn start_gated_request(addr: std::net::SocketAddr) -> tokio::net::TcpStream {
        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connects to the test listener");
        stream
            .write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("writes the gated request");
        stream
    }

    #[tokio::test]
    async fn receiver_adapter_handles_an_existing_edge_and_a_closed_channel() {
        let (tx, rx) = watch::channel(true);
        wait_for_shutdown(rx).await;

        let (tx_closed, rx_closed) = watch::channel(false);
        drop(tx_closed);
        wait_for_shutdown(rx_closed).await;

        drop(tx);
    }

    #[tokio::test]
    async fn admitted_request_completes_after_readiness_starts_draining() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds a test listener");
        let addr = listener.local_addr().expect("reads the bound address");
        let gate = RequestGate {
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        };
        let app = Router::new()
            .route("/slow", get(gated_request))
            .with_state(gate.clone());
        let readiness = Readiness::new();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server_readiness = readiness.clone();
        let server = tokio::spawn(async move {
            serve_until(
                listener,
                app,
                server_readiness,
                Duration::from_secs(30),
                async move {
                    let _ = shutdown_rx.await;
                },
            )
            .await
        });

        let mut stream = start_gated_request(addr).await;
        gate.entered.notified().await;
        shutdown_tx.send(()).expect("delivers the shutdown edge");
        tokio::task::yield_now().await;
        assert_eq!(readiness.status(), ReadinessStatus::Draining);
        assert!(
            !server.is_finished(),
            "the admitted request is still draining"
        );

        gate.release.notify_one();
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .expect("reads the completed response");
        assert!(
            String::from_utf8_lossy(&response).contains("completed"),
            "the response completed after the signal"
        );
        server
            .await
            .expect("server task joins")
            .expect("serve succeeds");
    }

    #[tokio::test(start_paused = true)]
    async fn drain_deadline_starts_at_the_shutdown_edge_and_bounds_a_stuck_request() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds a test listener");
        let addr = listener.local_addr().expect("reads the bound address");
        let gate = RequestGate {
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        };
        let app = Router::new()
            .route("/slow", get(gated_request))
            .with_state(gate.clone());
        let readiness = Readiness::new();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_until(
            listener,
            app,
            readiness.clone(),
            Duration::from_secs(10),
            async move {
                let _ = shutdown_rx.await;
            },
        ));

        let _stream = start_gated_request(addr).await;
        gate.entered.notified().await;
        tokio::time::advance(Duration::from_secs(100)).await;
        assert!(!server.is_finished(), "no deadline runs before the signal");

        shutdown_tx.send(()).expect("delivers the shutdown edge");
        tokio::task::yield_now().await;
        assert_eq!(readiness.status(), ReadinessStatus::Draining);
        tokio::time::advance(Duration::from_secs(9)).await;
        tokio::task::yield_now().await;
        assert!(!server.is_finished(), "the whole configured drain remains");

        tokio::time::advance(Duration::from_secs(1)).await;
        server
            .await
            .expect("server task joins at the deadline")
            .expect("a bounded forced drain is a clean process exit");
    }

    #[tokio::test]
    async fn serving_error_is_returned_to_the_process_owner() {
        let readiness = Readiness::new();
        let error = run_until_shutdown(
            future::ready(Err(io::Error::other("accept failed"))),
            readiness,
            Duration::from_secs(1),
            future::pending(),
        )
        .await
        .expect_err("a serving failure is not swallowed");

        assert!(error.to_string().contains("accept failed"));
    }

    #[tokio::test(start_paused = true)]
    async fn background_tasks_share_one_shutdown_deadline() {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let tasks = vec![
            tokio::spawn(future::pending()),
            tokio::spawn(future::pending()),
        ];
        let supervisor = tokio::spawn(supervise_tasks(tasks, shutdown_rx, Duration::from_secs(7)));

        tokio::time::advance(Duration::from_secs(50)).await;
        assert!(
            !supervisor.is_finished(),
            "no deadline runs before shutdown"
        );
        shutdown_tx.send(true).expect("delivers the shutdown edge");
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(6)).await;
        tokio::task::yield_now().await;
        assert!(
            !supervisor.is_finished(),
            "the shared deadline has not elapsed"
        );

        tokio::time::advance(Duration::from_secs(1)).await;
        supervisor.await.expect("supervisor exits at one deadline");
    }
}
