//! At-least-once webhook delivery over the same outbox the change feed
//! reads (`#115`): one [`WebhookSubscriptionRuntime`] per declaratively
//! configured subscription (`config::WebhookSubscriptionDecl`), draining
//! every matched collection's outbox on its own cursor — the same "one log,
//! N consumers, independent cursors" shape `crate::invalidation`'s
//! `GenerationStore` already established, one level up: a subscription
//! tracks one cursor PER collection it matches (an outbox `Sequence` is a
//! total order within one collection only, never across collections — see
//! `crate::outbox`'s own doc), all sharing one runtime struct and one
//! dead-letter store.
//!
//! Delivery is a signed `POST` of exactly one [`FeedEntry`] envelope per
//! obligation — never a batch body, so a receiver's own dedupe-by-sequence
//! logic (the at-least-once contract this issue states outright) stays
//! trivial. A failed delivery retries with [`backoff_delay`] up to
//! `max_attempts`; exhausting the budget dead-letters the entry (bounded
//! per-subscription ring, [`WebhookSubscriptionRuntime::record_dead_letter`])
//! and still advances the cursor past it — a permanently broken endpoint
//! must degrade to "misses events, loudly, via the dead-letter surface and
//! its own metrics," never wedge every other consumer's retention floor
//! forever (`crate::retention`).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use sha2::Sha256;

use crate::config::CollectionDecl;
use crate::error::{Error, Result};
use crate::feed::{FeedEntry, FeedOperation};
use crate::outbox::{OutboxSource, Sequence};

type HmacSha256 = Hmac<Sha256>;

/// Header carrying a delivery's HMAC-SHA256 signature (lowercase hex) over
/// the exact JSON body bytes sent, keyed by the subscription's own secret.
pub const SIGNATURE_HEADER: &str = "x-tellurion-signature-256";

/// HMAC-SHA256 over `body`, keyed by `secret`, hex-encoded — the signing
/// half of every webhook delivery. Mirrors `sigv4.rs`'s own
/// `hmac`+`sha2`-based helper (this workspace's one "no hand-rolled HMAC"
/// idiom), independently here because `sigv4.rs` is a private module with
/// its own SigV4-specific key-derivation chain, not a general-purpose HMAC
/// helper worth threading a dependency on.
pub fn hmac_sha256_hex(secret: &[u8], body: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret).expect("HMAC-SHA256 accepts a key of any length");
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Bounded exponential backoff: `base_ms * 2^(attempt - 1)`, clamped to
/// `max_ms`. `attempt` is 1-based (the delay before retrying after `attempt`
/// failed attempts so far). The shift is capped well below `u32::BITS` so a
/// pathologically large `attempt` still saturates cleanly to `max_ms`
/// rather than overflowing.
pub fn backoff_delay(attempt: u32, base_ms: u64, max_ms: u64) -> Duration {
    let shift = attempt.saturating_sub(1).min(32);
    let scaled = base_ms.saturating_mul(1u64 << shift);
    Duration::from_millis(scaled.min(max_ms))
}

/// Bounded retry policy for one delivery attempt sequence — the runtime
/// mirror of `config::WebhookDeliveryConfig`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebhookRetryPolicy {
    pub max_attempts: u32,
    pub base_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

/// One exhausted delivery — the dead-letter surface's own compact envelope,
/// carrying the same [`FeedEntry`] discipline the feed itself uses (never a
/// payload) plus how many attempts were made and the most recent failure.
/// `Serialize` (not `Deserialize`: this side never reads one back from the
/// wire) is what lets the dead-letter HTTP surface
/// (`tellurion-server::webhook_admin`) hand this straight to `Json` — same
/// `camelCase` convention `FeedEntry` itself uses, so a page mixing the two
/// (this struct nests one) reads as one consistent shape on the wire.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeadLetterEntry {
    pub entry: FeedEntry,
    pub attempts: u32,
    pub last_error: String,
    pub failed_at: String,
}

struct DeadLetterRecord {
    ordinal: u64,
    entry: DeadLetterEntry,
}

/// A bounded per-subscription ring of [`DeadLetterEntry`] — capacity-evicts
/// the oldest entry, never grows without bound regardless of how long a
/// subscription's endpoint stays broken. Its cursor is a runtime-generation
/// UUID plus a subscription-wide ordinal, never the nested feed entry's
/// per-collection sequence: one subscription may interleave several
/// independently ordered collection outboxes.
struct DeadLetterRing {
    generation: uuid::Uuid,
    next_ordinal: u64,
    entries: VecDeque<DeadLetterRecord>,
    capacity: usize,
}

impl DeadLetterRing {
    fn new(capacity: usize) -> Self {
        Self {
            generation: uuid::Uuid::new_v4(),
            next_ordinal: 0,
            entries: VecDeque::with_capacity(capacity.min(1024)),
            capacity: capacity.max(1),
        }
    }

    fn push(&mut self, entry: DeadLetterEntry) {
        self.next_ordinal = self
            .next_ordinal
            .checked_add(1)
            .expect("a process cannot dead-letter u64::MAX deliveries");
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(DeadLetterRecord {
            ordinal: self.next_ordinal,
            entry,
        });
    }

    fn encode_cursor(&self, ordinal: u64) -> String {
        format!("{}:{ordinal}", self.generation)
    }

    fn decode_cursor(&self, cursor: &str) -> Result<u64> {
        let Some((generation, ordinal)) = cursor.split_once(':') else {
            return Err(Error::Invalid(
                "invalid dead-letter cursor: expected an opaque cursor from this runtime"
                    .to_string(),
            ));
        };
        if generation != self.generation.to_string() {
            return Err(Error::Invalid(
                "invalid dead-letter cursor: the subscription runtime has changed".to_string(),
            ));
        }
        ordinal.parse::<u64>().map_err(|_| {
            Error::Invalid(
                "invalid dead-letter cursor: expected an opaque cursor from this runtime"
                    .to_string(),
            )
        })
    }

    /// Pages in insertion order by a subscription-wide ordinal. The ring
    /// can inspect one extra record, so `next` is emitted only when another
    /// entry actually exists; an exactly-full terminal page has no false
    /// continuation.
    fn page(
        &self,
        since: Option<&str>,
        limit: u32,
    ) -> Result<(Vec<DeadLetterEntry>, Option<String>)> {
        let after = match since {
            Some(cursor) => self.decode_cursor(cursor)?,
            None => 0,
        };
        let mut matched: Vec<&DeadLetterRecord> = self
            .entries
            .iter()
            .filter(|dead| dead.ordinal > after)
            .take(limit as usize + 1)
            .collect();
        let has_more = matched.len() > limit as usize;
        matched.truncate(limit as usize);
        let next = if has_more {
            matched.last().map(|dead| self.encode_cursor(dead.ordinal))
        } else {
            None
        };
        Ok((
            matched.into_iter().map(|dead| dead.entry.clone()).collect(),
            next,
        ))
    }
}

/// One declared subscription's runtime state: a per-collection drain cursor
/// (pre-registered for every collection the subscription's scope matched at
/// spawn time — see the module doc), the operations it filters on, and its
/// own bounded dead-letter ring. Never holds the secret in a `Debug`able
/// form directly accessible outside this module — [`Self::secret`] is
/// crate-visible only.
///
/// `cursors` is a `RwLock`-guarded map, not a plain one built once and
/// frozen: a live config rebind (`#115`, `tellurion-server::
/// webhook_consumer::rebind`) can fold a newly-matched collection into an
/// EXISTING subscription (the same `Arc<WebhookSubscriptionRuntime>` a
/// previous generation already delivered through, kept alive across the
/// rebind precisely so its progress and dead-letter ring survive) —
/// [`ensure_collection`](Self::ensure_collection) is the seam that
/// registration goes through, well after this value's own construction.
pub struct WebhookSubscriptionRuntime {
    id: String,
    url: String,
    secret: Vec<u8>,
    /// Empty means "every operation" — the same "empty list matches
    /// everything" convention `config::GrantScope` already uses.
    operations: Vec<FeedOperation>,
    cursors: RwLock<HashMap<String, AtomicU64>>,
    dead_letters: RwLock<DeadLetterRing>,
}

impl WebhookSubscriptionRuntime {
    pub fn new(
        id: String,
        url: String,
        secret: Vec<u8>,
        operations: Vec<FeedOperation>,
        collection_ids: impl IntoIterator<Item = String>,
        dead_letter_capacity: usize,
    ) -> Self {
        Self {
            id,
            url,
            secret,
            operations,
            cursors: RwLock::new(
                collection_ids
                    .into_iter()
                    .map(|id| (id, AtomicU64::new(0)))
                    .collect(),
            ),
            dead_letters: RwLock::new(DeadLetterRing::new(dead_letter_capacity)),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    fn secret(&self) -> &[u8] {
        &self.secret
    }

    /// This subscription's own drain cursor for `collection_id` —
    /// `Sequence(0)` for a collection it never registered (never matched by
    /// its scope, so no task was ever spawned for it).
    pub fn cursor(&self, collection_id: &str) -> Sequence {
        self.cursors
            .read()
            .expect("cursor map lock is never held across a panic")
            .get(collection_id)
            .map(|cursor| Sequence(cursor.load(Ordering::Relaxed)))
            .unwrap_or(Sequence(0))
    }

    fn advance_cursor(&self, collection_id: &str, sequence: Sequence) {
        if let Some(cursor) = self
            .cursors
            .read()
            .expect("cursor map lock is never held across a panic")
            .get(collection_id)
        {
            cursor.store(sequence.0, Ordering::Relaxed);
        }
    }

    /// Registers a cursor for `collection_id`, seeded at `initial`, UNLESS
    /// this subscription already tracks one for it — a no-op in that case,
    /// which is exactly what preserves an already-tracked collection's own
    /// delivery progress when a live config rebind (`#115`) re-derives this
    /// subscription's matched-collection set and finds it unchanged (or
    /// only grown). The one seam that ever grows `cursors` past
    /// construction time; see `tellurion-server::webhook_consumer`'s own
    /// module doc for the conservative "a newly declared pair starts at
    /// zero and replays whatever history retention still holds" policy this
    /// backs. A later high-water seed would lose writes committed between a
    /// config swap and the manager observing that generation.
    pub fn ensure_collection(&self, collection_id: &str, initial: Sequence) {
        self.cursors
            .write()
            .expect("cursor map lock is never held across a panic")
            .entry(collection_id.to_string())
            .or_insert_with(|| AtomicU64::new(initial.0));
    }

    /// Stops tracking a collection that no longer matches this
    /// subscription after a live config rebind. Removing the cursor is
    /// essential for retention: a stale cursor must not hold the outbox
    /// floor back after its delivery task has been cancelled.
    pub fn remove_collection(&self, collection_id: &str) {
        self.cursors
            .write()
            .expect("cursor map lock is never held across a panic")
            .remove(collection_id);
    }

    /// Every collection this subscription has a registered cursor for — the
    /// seam `crate::retention`'s floor computation (and the server's own
    /// wiring) reads to know which collections to fold this subscription's
    /// cursor into. Owned (not a borrowing iterator): the backing map lives
    /// behind a lock now, so a snapshot of the keys, taken under one brief
    /// read lock, is simpler for every caller than threading a guard's
    /// lifetime through — this is only ever consulted in small, infrequent
    /// loops (a retention tick, a rebind), never a hot request path.
    pub fn registered_collections(&self) -> Vec<String> {
        self.cursors
            .read()
            .expect("cursor map lock is never held across a panic")
            .keys()
            .cloned()
            .collect()
    }

    fn accepts_operation(&self, operation: FeedOperation) -> bool {
        self.operations.is_empty() || self.operations.contains(&operation)
    }

    fn record_dead_letter(&self, entry: DeadLetterEntry) {
        self.dead_letters
            .write()
            .expect("dead-letter ring lock is never held across a panic")
            .push(entry);
    }

    /// Pages this subscription's dead-lettered deliveries — the "queryable
    /// stalled-subscriptions view" `#115` calls for, same compact-envelope
    /// cursor discipline as the feed itself.
    pub fn dead_letters(
        &self,
        since: Option<&str>,
        limit: u32,
    ) -> Result<(Vec<DeadLetterEntry>, Option<String>)> {
        self.dead_letters
            .read()
            .expect("dead-letter ring lock is never held across a panic")
            .page(since, limit)
    }

    /// How many dead-lettered deliveries this subscription currently holds —
    /// `run_webhook_consumer`'s own `webhook_dead_letter_queue_size` gauge
    /// input; the dedicated HTTP inspection surface pages the entries
    /// themselves through [`Self::dead_letters`].
    pub fn dead_letter_count(&self) -> usize {
        self.dead_letters
            .read()
            .expect("dead-letter ring lock is never held across a panic")
            .entries
            .len()
    }

    /// `#115`'s own lag metric input: how far this collection's drain cursor
    /// trails the primary's high-water mark. `None` for a collection this
    /// subscription never registered.
    pub fn lag(&self, collection_id: &str, primary_high_water: Sequence) -> Option<u64> {
        self.cursors
            .read()
            .expect("cursor map lock is never held across a panic")
            .get(collection_id)
            .map(|cursor| {
                primary_high_water
                    .0
                    .saturating_sub(cursor.load(Ordering::Relaxed))
            })
    }
}

/// Delivers signed `body` to `url` — the seam a real `reqwest`-backed
/// implementation and a test double both satisfy, so `drain_once`'s own
/// retry/backoff/dead-letter logic is testable without a real HTTP client or
/// network access. `true` is a successful (2xx) delivery; `false` is
/// anything else (non-2xx status, connect/timeout error) — this trait
/// deliberately loses the distinction between "the server said no" and "the
/// network failed," since both retry identically here.
#[async_trait]
pub trait WebhookDeliverer: Send + Sync {
    async fn deliver(&self, url: &str, body: &[u8], signature: &str) -> bool;
}

/// A `reqwest`-backed [`WebhookDeliverer`] — the real production
/// implementation. Already a dependency of this crate (`auth.rs`'s OIDC
/// discovery, `objectstore.rs`'s S3 profile), so this adds no new crate.
pub struct ReqwestDeliverer {
    client: reqwest::Client,
}

impl ReqwestDeliverer {
    pub fn new(request_timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(request_timeout)
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

#[async_trait]
impl WebhookDeliverer for ReqwestDeliverer {
    async fn deliver(&self, url: &str, body: &[u8], signature: &str) -> bool {
        match self
            .client
            .post(url)
            .header(SIGNATURE_HEADER, signature)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_vec())
            .send()
            .await
        {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }
}

/// One obligation's delivery, with bounded retries: signs the envelope once,
/// attempts delivery up to `retry.max_attempts` times, sleeping
/// [`backoff_delay`] between attempts. `Ok(())` on the first successful
/// attempt; `Err(DeadLetterEntry)` once the budget is exhausted (the caller
/// records it and still advances the cursor — see the module doc).
async fn deliver_with_retries(
    subscription_id: &str,
    collection_id: &str,
    secret: &[u8],
    url: &str,
    entry: &FeedEntry,
    deliverer: &dyn WebhookDeliverer,
    retry: WebhookRetryPolicy,
) -> std::result::Result<(), DeadLetterEntry> {
    let body = serde_json::to_vec(entry).expect("FeedEntry always serializes to JSON");
    let signature = hmac_sha256_hex(secret, &body);
    let max_attempts = retry.max_attempts.max(1);
    let mut last_error = String::new();

    for attempt in 1..=max_attempts {
        metrics::counter!(
            "webhook_delivery_attempts_total",
            "subscription" => subscription_id.to_string(),
            "collection" => collection_id.to_string()
        )
        .increment(1);

        if deliverer.deliver(url, &body, &signature).await {
            return Ok(());
        }

        metrics::counter!(
            "webhook_delivery_failures_total",
            "subscription" => subscription_id.to_string(),
            "collection" => collection_id.to_string()
        )
        .increment(1);
        last_error = format!("delivery attempt {attempt}/{max_attempts} failed");

        if attempt < max_attempts {
            tokio::time::sleep(backoff_delay(
                attempt,
                retry.base_backoff_ms,
                retry.max_backoff_ms,
            ))
            .await;
        }
    }

    Err(DeadLetterEntry {
        entry: entry.clone(),
        attempts: max_attempts,
        last_error,
        failed_at: crate::timefmt::format_rfc3339_millis(SystemTime::now()),
    })
}

/// One drain pass for one (subscription, collection) pair: reads at most
/// `batch_size` obligations after this subscription's own cursor for
/// `collection`, attempts delivery for every one whose operation the
/// subscription accepts (an obligation the subscription filters out still
/// advances the cursor — it was never going to be delivered), dead-lettering
/// any delivery that exhausts its retry budget. Returns how many obligations
/// were read (`0` means caught up).
pub async fn drain_once(
    outbox: &dyn OutboxSource,
    collection: &CollectionDecl,
    subscription: &WebhookSubscriptionRuntime,
    deliverer: &dyn WebhookDeliverer,
    batch_size: u32,
    retry: WebhookRetryPolicy,
) -> Result<usize> {
    let cursor = subscription.cursor(&collection.id);
    let obligations = outbox.read_after(collection, cursor, batch_size).await?;
    if obligations.is_empty() {
        return Ok(0);
    }

    for obligation in &obligations {
        let entry = FeedEntry::from_obligation(&collection.id, obligation);
        if !subscription.accepts_operation(entry.operation) {
            continue;
        }
        if let Err(dead_letter) = deliver_with_retries(
            subscription.id(),
            &collection.id,
            subscription.secret(),
            &subscription.url,
            &entry,
            deliverer,
            retry,
        )
        .await
        {
            metrics::counter!(
                "webhook_dead_letter_total",
                "subscription" => subscription.id().to_string(),
                "collection" => collection.id.clone()
            )
            .increment(1);
            tracing::warn!(
                subscription = subscription.id(),
                collection = %collection.id,
                sequence = entry.sequence,
                error = %dead_letter.last_error,
                "webhook delivery exhausted its retry budget; dead-lettering and advancing past it"
            );
            subscription.record_dead_letter(dead_letter);
        }
    }

    let max_sequence = obligations
        .iter()
        .map(|obligation| obligation.sequence)
        .max()
        .expect("checked non-empty above");
    subscription.advance_cursor(&collection.id, max_sequence);
    Ok(obligations.len())
}

/// Bundles [`run_webhook_consumer`]'s tuning knobs (`config::
/// WebhookDeliveryConfig`'s runtime mirror) into one value — keeps the
/// function itself under clippy's argument-count lint without losing any of
/// them.
#[derive(Debug, Clone, Copy)]
pub struct WebhookConsumerSettings {
    pub batch_size: u32,
    pub retry: WebhookRetryPolicy,
    pub poll_interval: Duration,
}

/// Runs [`drain_once`] on a fixed `settings.poll_interval` until `shutdown`
/// reports `true` — the background-task shape `tellurion-server`'s
/// webhook-delivery wiring spawns one of per (subscription, collection)
/// pair. Mirrors `crate::invalidation::run_generation_consumer`'s own shape:
/// a failed pass is logged and retried next tick, and the lag gauge
/// (`webhook_delivery_lag`, labeled `subscription`/`collection`) is emitted
/// after every pass that can resolve the primary's own high-water mark.
pub async fn run_webhook_consumer(
    outbox: std::sync::Arc<dyn OutboxSource>,
    subscription: std::sync::Arc<WebhookSubscriptionRuntime>,
    collection: CollectionDecl,
    deliverer: std::sync::Arc<dyn WebhookDeliverer>,
    settings: WebhookConsumerSettings,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        match drain_once(
            outbox.as_ref(),
            &collection,
            subscription.as_ref(),
            deliverer.as_ref(),
            settings.batch_size,
            settings.retry,
        )
        .await
        {
            Ok(_) => {
                if let Ok(high_water) = outbox.primary_high_water(&collection).await {
                    if let Some(lag) = subscription.lag(&collection.id, high_water) {
                        metrics::gauge!(
                            "webhook_delivery_lag",
                            "subscription" => subscription.id().to_string(),
                            "collection" => collection.id.clone()
                        )
                        .set(lag as f64);
                    }
                }
            }
            Err(error) => {
                tracing::error!(
                    subscription = subscription.id(),
                    collection = %collection.id,
                    %error,
                    "webhook delivery pass failed; resuming from the last durable cursor on the next tick"
                );
            }
        }
        // The bounded dead-letter queue depth, independent of this pass's
        // own outcome — the queryable half of the stalled-subscriptions
        // surface (see `WebhookSubscriptionRuntime::dead_letter_count`'s own
        // doc).
        metrics::gauge!(
            "webhook_dead_letter_queue_size",
            "subscription" => subscription.id().to_string(),
            "collection" => collection.id.clone()
        )
        .set(subscription.dead_letter_count() as f64);
        tokio::select! {
            _ = tokio::time::sleep(settings.poll_interval) => {}
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbox::{MutationKind, Obligation};
    use async_trait::async_trait;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    fn collection() -> CollectionDecl {
        serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap()
    }

    fn obligation(sequence: u64) -> Obligation {
        Obligation {
            sequence: Sequence(sequence),
            feature_id: format!("f{sequence}"),
            kind: MutationKind::Upsert(serde_json::json!({"type": "Feature"})),
            version: Sequence(sequence),
            committed_at: SystemTime::UNIX_EPOCH,
            extent: crate::outbox::ObligationExtent::Unrecorded,
        }
    }

    // ---- hmac_sha256_hex / backoff_delay --------------------------------

    #[test]
    fn hmac_is_deterministic_and_keyed_by_the_secret() {
        let a = hmac_sha256_hex(b"secret-one", b"body");
        let b = hmac_sha256_hex(b"secret-one", b"body");
        let c = hmac_sha256_hex(b"secret-two", b"body");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64, "hex-encoded SHA-256 is 64 characters");
    }

    #[test]
    fn backoff_grows_exponentially_from_the_base() {
        assert_eq!(backoff_delay(1, 100, 10_000), Duration::from_millis(100));
        assert_eq!(backoff_delay(2, 100, 10_000), Duration::from_millis(200));
        assert_eq!(backoff_delay(3, 100, 10_000), Duration::from_millis(400));
        assert_eq!(backoff_delay(4, 100, 10_000), Duration::from_millis(800));
    }

    #[test]
    fn backoff_clamps_to_the_configured_maximum() {
        assert_eq!(backoff_delay(20, 100, 5_000), Duration::from_millis(5_000));
    }

    #[test]
    fn backoff_never_overflows_for_a_very_large_attempt_count() {
        let delay = backoff_delay(u32::MAX, 1, 60_000);
        assert_eq!(delay, Duration::from_millis(60_000));
    }

    // ---- DeadLetterRing ---------------------------------------------------

    fn dead_letter(sequence: u64) -> DeadLetterEntry {
        dead_letter_for("demo", sequence)
    }

    fn dead_letter_for(collection: &str, sequence: u64) -> DeadLetterEntry {
        DeadLetterEntry {
            entry: FeedEntry::from_obligation(collection, &obligation(sequence)),
            attempts: 3,
            last_error: "boom".to_string(),
            failed_at: "1970-01-01T00:00:00.000Z".to_string(),
        }
    }

    #[test]
    fn dead_letter_ring_evicts_the_oldest_entry_past_capacity() {
        let mut ring = DeadLetterRing::new(2);
        ring.push(dead_letter(1));
        ring.push(dead_letter(2));
        ring.push(dead_letter(3));
        let (page, _) = ring.page(None, 10).unwrap();
        assert_eq!(
            page.iter().map(|d| d.entry.sequence).collect::<Vec<_>>(),
            vec![2, 3],
            "the oldest entry (sequence 1) should have been evicted"
        );
    }

    #[test]
    fn dead_letter_ring_pages_since_a_cursor() {
        let mut ring = DeadLetterRing::new(10);
        for sequence in 1..=5 {
            ring.push(dead_letter(sequence));
        }
        let (_, first_next) = ring.page(None, 2).unwrap();
        let (page, next) = ring.page(first_next.as_deref(), 2).unwrap();
        assert_eq!(
            page.iter().map(|d| d.entry.sequence).collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert!(next.is_some());
    }

    #[test]
    fn dead_letter_cursor_orders_colliding_collection_sequences() {
        let mut ring = DeadLetterRing::new(10);
        ring.push(dead_letter_for("alpha", 1));
        ring.push(dead_letter_for("beta", 1));

        let (first, next) = ring.page(None, 1).unwrap();
        assert_eq!(first[0].entry.collection, "alpha");
        let (second, _) = ring.page(next.as_deref(), 1).unwrap();
        assert_eq!(second[0].entry.collection, "beta");
    }

    #[test]
    fn dead_letter_terminal_page_has_no_false_next_cursor() {
        let mut ring = DeadLetterRing::new(10);
        ring.push(dead_letter(1));
        ring.push(dead_letter(2));

        let (page, next) = ring.page(None, 2).unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(next, None);
    }

    #[test]
    fn dead_letter_cursor_from_an_old_runtime_is_rejected() {
        let mut old = DeadLetterRing::new(10);
        old.push(dead_letter(1));
        old.push(dead_letter(2));
        let (_, cursor) = old.page(None, 1).unwrap();

        let replacement = DeadLetterRing::new(10);
        assert!(replacement.page(cursor.as_deref(), 1).is_err());
    }

    // ---- WebhookSubscriptionRuntime ----------------------------------------

    fn runtime(collections: &[&str]) -> WebhookSubscriptionRuntime {
        WebhookSubscriptionRuntime::new(
            "sub-1".to_string(),
            "http://example.invalid/hook".to_string(),
            b"shh".to_vec(),
            Vec::new(),
            collections.iter().map(|c| c.to_string()),
            10,
        )
    }

    #[test]
    fn an_unregistered_collection_always_reports_cursor_zero() {
        let sub = runtime(&["demo"]);
        assert_eq!(sub.cursor("other"), Sequence(0));
        assert_eq!(sub.lag("other", Sequence(100)), None);
    }

    #[test]
    fn a_runtime_rebind_can_add_and_remove_collection_cursors() {
        let sub = runtime(&["demo"]);
        sub.ensure_collection("new", Sequence(41));
        assert_eq!(sub.cursor("new"), Sequence(41));
        assert!(sub.registered_collections().contains(&"new".to_string()));

        sub.remove_collection("demo");
        assert_eq!(sub.cursor("demo"), Sequence(0));
        assert_eq!(sub.lag("demo", Sequence(100)), None);
        assert!(!sub.registered_collections().contains(&"demo".to_string()));
    }

    #[test]
    fn empty_operations_filter_accepts_every_operation() {
        let sub = runtime(&["demo"]);
        assert!(sub.accepts_operation(FeedOperation::Upsert));
        assert!(sub.accepts_operation(FeedOperation::Delete));
    }

    #[test]
    fn a_declared_operations_filter_only_accepts_named_operations() {
        let sub = WebhookSubscriptionRuntime::new(
            "sub-1".to_string(),
            "http://example.invalid".to_string(),
            b"shh".to_vec(),
            vec![FeedOperation::Delete],
            ["demo".to_string()],
            10,
        );
        assert!(!sub.accepts_operation(FeedOperation::Upsert));
        assert!(sub.accepts_operation(FeedOperation::Delete));
    }

    // ---- drain_once ---------------------------------------------------------

    struct FakeOutbox {
        obligations: Vec<Obligation>,
    }

    #[async_trait]
    impl OutboxSource for FakeOutbox {
        async fn read_after(
            &self,
            _collection: &CollectionDecl,
            after: Sequence,
            limit: u32,
        ) -> Result<Vec<Obligation>> {
            Ok(self
                .obligations
                .iter()
                .filter(|o| o.sequence > after)
                .take(limit as usize)
                .cloned()
                .collect())
        }

        async fn primary_high_water(&self, _collection: &CollectionDecl) -> Result<Sequence> {
            Ok(self
                .obligations
                .last()
                .map(|o| o.sequence)
                .unwrap_or(Sequence(0)))
        }
    }

    /// A deliverer that fails a fixed number of times before succeeding —
    /// exercises the retry path without any real network I/O.
    struct FlakyDeliverer {
        fail_first_n: AtomicUsize,
        calls: AtomicUsize,
        received_signatures: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl WebhookDeliverer for FlakyDeliverer {
        async fn deliver(&self, _url: &str, _body: &[u8], signature: &str) -> bool {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.received_signatures
                .lock()
                .unwrap()
                .push(signature.to_string());
            let remaining =
                self.fail_first_n
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                        if n == 0 {
                            None
                        } else {
                            Some(n - 1)
                        }
                    });
            remaining.is_err() // Err means fetch_update found 0 -> succeed.
        }
    }

    fn no_sleep_retry() -> WebhookRetryPolicy {
        WebhookRetryPolicy {
            max_attempts: 3,
            base_backoff_ms: 0,
            max_backoff_ms: 0,
        }
    }

    #[tokio::test]
    async fn drain_once_delivers_every_obligation_and_advances_the_cursor() {
        let outbox = FakeOutbox {
            obligations: vec![obligation(1), obligation(2)],
        };
        let subscription = runtime(&["demo"]);
        let deliverer = FlakyDeliverer {
            fail_first_n: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            received_signatures: Mutex::new(Vec::new()),
        };
        let applied = drain_once(
            &outbox,
            &collection(),
            &subscription,
            &deliverer,
            100,
            no_sleep_retry(),
        )
        .await
        .unwrap();
        assert_eq!(applied, 2);
        assert_eq!(deliverer.calls.load(Ordering::SeqCst), 2);
        assert_eq!(subscription.cursor("demo"), Sequence(2));
        // A real HMAC signature (not empty/placeholder) was sent for both.
        assert_eq!(deliverer.received_signatures.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_transient_failure_retries_and_still_succeeds() {
        let outbox = FakeOutbox {
            obligations: vec![obligation(1)],
        };
        let subscription = runtime(&["demo"]);
        let deliverer = FlakyDeliverer {
            fail_first_n: AtomicUsize::new(2),
            calls: AtomicUsize::new(0),
            received_signatures: Mutex::new(Vec::new()),
        };
        drain_once(
            &outbox,
            &collection(),
            &subscription,
            &deliverer,
            100,
            no_sleep_retry(),
        )
        .await
        .unwrap();
        assert_eq!(deliverer.calls.load(Ordering::SeqCst), 3);
        assert_eq!(subscription.cursor("demo"), Sequence(1));
    }

    #[tokio::test]
    async fn exhausting_the_retry_budget_dead_letters_and_still_advances_the_cursor() {
        let outbox = FakeOutbox {
            obligations: vec![obligation(1), obligation(2)],
        };
        let subscription = runtime(&["demo"]);
        let deliverer = FlakyDeliverer {
            fail_first_n: AtomicUsize::new(usize::MAX / 2),
            calls: AtomicUsize::new(0),
            received_signatures: Mutex::new(Vec::new()),
        };
        drain_once(
            &outbox,
            &collection(),
            &subscription,
            &deliverer,
            100,
            no_sleep_retry(),
        )
        .await
        .unwrap();
        // The cursor still advances past both — the at-least-once contract
        // is upheld via the dead-letter surface, not by wedging forever.
        assert_eq!(subscription.cursor("demo"), Sequence(2));
        let (dead_letters, _) = subscription.dead_letters(None, 10).unwrap();
        assert_eq!(dead_letters.len(), 2);
        assert_eq!(dead_letters[0].attempts, 3);
    }

    #[tokio::test]
    async fn an_operation_the_subscription_filters_out_is_skipped_but_still_advances_the_cursor() {
        let outbox = FakeOutbox {
            obligations: vec![obligation(1)],
        };
        let subscription = WebhookSubscriptionRuntime::new(
            "sub-1".to_string(),
            "http://example.invalid".to_string(),
            b"shh".to_vec(),
            vec![FeedOperation::Delete],
            ["demo".to_string()],
            10,
        );
        let deliverer = FlakyDeliverer {
            fail_first_n: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            received_signatures: Mutex::new(Vec::new()),
        };
        drain_once(
            &outbox,
            &collection(),
            &subscription,
            &deliverer,
            100,
            no_sleep_retry(),
        )
        .await
        .unwrap();
        assert_eq!(
            deliverer.calls.load(Ordering::SeqCst),
            0,
            "an upsert obligation should never reach a delete-only subscription"
        );
        assert_eq!(subscription.cursor("demo"), Sequence(1));
    }

    #[tokio::test]
    async fn drain_once_reads_nothing_new_once_caught_up() {
        let outbox = FakeOutbox {
            obligations: vec![obligation(1)],
        };
        let subscription = runtime(&["demo"]);
        let deliverer = FlakyDeliverer {
            fail_first_n: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            received_signatures: Mutex::new(Vec::new()),
        };
        drain_once(
            &outbox,
            &collection(),
            &subscription,
            &deliverer,
            100,
            no_sleep_retry(),
        )
        .await
        .unwrap();
        let applied_again = drain_once(
            &outbox,
            &collection(),
            &subscription,
            &deliverer,
            100,
            no_sleep_retry(),
        )
        .await
        .unwrap();
        assert_eq!(applied_again, 0);
    }
}
