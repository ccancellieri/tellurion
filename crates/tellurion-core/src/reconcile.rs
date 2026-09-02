//! The reconcile surface (assets-and-object-storage proposal, `#93`): a
//! **read-only** drift report between a collection's asset records and its
//! object store — no repair action, no deletion, no state flip. Two
//! directions, both named with enough identity to act on later:
//!
//! - **broken**: a record in [`crate::AssetState::Available`] whose object
//!   is missing from the store — the record thinks the bytes exist, the
//!   store disagrees.
//! - **orphaned**: something present in the store's own managed namespace
//!   ([`ListableObjectStore::list_all`]) with no record — for any state —
//!   that claims it, including a leftover resumable-upload `.upload`
//!   staging file (`crate::objectstore::FsObjectStore`'s own doc) nobody
//!   ever completed or abandoned.
//!
//! Deliberately narrower than the proposal's own three-way drift
//! description (`pending` past a TTL is the third): that is an
//! abandoned-upload sweep, a different, time-based surface — this report
//! only ever compares "what the database says" against "what the store
//! actually has", which needs no clock at all.
//!
//! `reconcile` never mutates either side — [`crate::AssetRecordStore`]'s own
//! `list` and [`ListableObjectStore::list_all`] are both read-only, and
//! nothing here calls anything else.

use uuid::Uuid;

use crate::asset::{AssetKind, AssetRecordStore, AssetState};
use crate::config::CollectionDecl;
use crate::error::{Error, Result};
use crate::objectstore::ListableObjectStore;

/// An [`crate::AssetState::Available`] managed record whose object the
/// store no longer has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokenAsset {
    pub item_id: Option<String>,
    pub key: String,
    pub id: Uuid,
}

/// Something in the store's managed namespace with no record (of any
/// state) claiming it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanedObject {
    /// The store's own raw entry name (`ListedObject::raw_name`) — kept
    /// verbatim so an entry with no parseable [`Uuid`] (`id: None`) still
    /// carries enough identity to find and remove by hand.
    pub raw_name: String,
    pub id: Option<Uuid>,
    pub is_staging: bool,
}

/// The full report [`reconcile`] returns — empty on a consistent store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub broken: Vec<BrokenAsset>,
    pub orphaned: Vec<OrphanedObject>,
}

impl ReconcileReport {
    pub fn is_clean(&self) -> bool {
        self.broken.is_empty() && self.orphaned.is_empty()
    }
}

/// Walks `collection`'s asset records ([`AssetRecordStore::list`]) and its
/// object store's managed namespace ([`ListableObjectStore::list_all`]) and
/// reports drift both ways — see this module's own doc for exactly what
/// "drift" means here. Read-only: neither input is ever written to.
pub async fn reconcile(
    store: &dyn AssetRecordStore,
    objects: &dyn ListableObjectStore,
    collection: &CollectionDecl,
) -> Result<ReconcileReport> {
    let records = store.list(collection).await?;
    let listed = objects
        .list_all()
        .await
        .map_err(|err| Error::Storage(Box::new(err)))?;

    // Every internal id a managed record — pending, available, or failed —
    // currently claims. A resumable-upload staging file for a still-pending
    // asset is expected, legitimate machinery, not an orphan; a completed
    // object matching ANY managed record's id, whatever its state, is
    // likewise not this report's concern (a state disagreement in that
    // direction is not one of the two drifts this surface names — see this
    // module's own doc).
    let known_ids: std::collections::HashSet<Uuid> = records
        .iter()
        .filter(|entry| entry.record.kind == AssetKind::Managed)
        .map(|entry| entry.record.id)
        .collect();

    let broken = records
        .iter()
        .filter(|entry| {
            entry.record.kind == AssetKind::Managed && entry.record.state == AssetState::Available
        })
        .filter(|entry| {
            !listed
                .iter()
                .any(|obj| !obj.is_staging && obj.id == Some(entry.record.id))
        })
        .map(|entry| BrokenAsset {
            item_id: entry.item_id.clone(),
            key: entry.key.clone(),
            id: entry.record.id,
        })
        .collect();

    let orphaned = listed
        .into_iter()
        .filter(|obj| !matches!(obj.id, Some(id) if known_ids.contains(&id)))
        .map(|obj| OrphanedObject {
            raw_name: obj.raw_name,
            id: obj.id,
            is_staging: obj.is_staging,
        })
        .collect();

    Ok(ReconcileReport { broken, orphaned })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{
        register_managed, AssetPolicy, Digest, InMemoryAssetRecordStore, RegisterManagedRequest,
    };
    use crate::objectstore::{InMemoryObjectStore, ObjectKey, ObjectStore as _};

    fn collection() -> CollectionDecl {
        serde_yaml::from_str(
            r#"
id: demo
catalog: default
storage: main
table: demo
"#,
        )
        .unwrap()
    }

    fn open_policy() -> AssetPolicy<'static> {
        AssetPolicy {
            max_asset_bytes: 1_000_000,
            allowed_media_types: None,
        }
    }

    async fn register_and_upload(
        store: &InMemoryAssetRecordStore,
        objects: &InMemoryObjectStore,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
        bytes: &'static [u8],
    ) -> Uuid {
        let digest = compute_digest(bytes);
        let record = register_managed(
            store,
            &open_policy(),
            collection,
            item_id,
            key,
            RegisterManagedRequest {
                media_type: Some("application/octet-stream".to_string()),
                title: None,
                description: None,
                roles: vec![],
                declared_size: bytes.len() as u64,
                digest,
            },
        )
        .await
        .unwrap();
        objects
            .put(ObjectKey::new(record.id), bytes::Bytes::from_static(bytes))
            .await
            .unwrap();
        store
            .finalize(
                collection,
                item_id,
                key,
                crate::asset::FinalizeOutcome::Available,
            )
            .await
            .unwrap();
        record.id
    }

    fn compute_digest(bytes: &[u8]) -> Digest {
        crate::asset::compute_sha256(bytes)
    }

    #[tokio::test]
    async fn empty_on_a_fully_consistent_store() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        register_and_upload(&store, &objects, &collection, None, "thumb", b"hello").await;
        register_and_upload(
            &store,
            &objects,
            &collection,
            Some("feature-1"),
            "photo",
            b"world",
        )
        .await;

        let report = reconcile(&store, &objects, &collection).await.unwrap();
        assert!(report.is_clean(), "{report:?}");
    }

    #[tokio::test]
    async fn names_a_missing_object_as_broken() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let id = register_and_upload(
            &store,
            &objects,
            &collection,
            Some("feature-1"),
            "thumb",
            b"hello",
        )
        .await;

        // The store loses the object out from under the still-`available`
        // record — a bypassed delete, a lost bucket object, ... whatever
        // the cause, the record and the store now disagree.
        objects.delete(ObjectKey::new(id)).await.unwrap();

        let report = reconcile(&store, &objects, &collection).await.unwrap();
        assert_eq!(report.broken.len(), 1);
        assert_eq!(report.broken[0].item_id.as_deref(), Some("feature-1"));
        assert_eq!(report.broken[0].key, "thumb");
        assert_eq!(report.broken[0].id, id);
        assert!(report.orphaned.is_empty());
    }

    #[tokio::test]
    async fn names_an_object_with_no_record_as_orphaned() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        register_and_upload(&store, &objects, &collection, None, "thumb", b"hello").await;

        // Written straight to the store, never registered — no record ever
        // claims this id.
        let stray = Uuid::new_v4();
        objects
            .put(ObjectKey::new(stray), bytes::Bytes::from_static(b"junk"))
            .await
            .unwrap();

        let report = reconcile(&store, &objects, &collection).await.unwrap();
        assert!(report.broken.is_empty());
        assert_eq!(report.orphaned.len(), 1);
        assert_eq!(report.orphaned[0].id, Some(stray));
        assert!(!report.orphaned[0].is_staging);
    }

    #[tokio::test]
    async fn names_a_leftover_resumable_upload_staging_file_as_orphaned() {
        use crate::objectstore::ResumableUploadStore as _;

        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();

        // A resumable upload created for an id with no (or no longer any)
        // pending record — the caller vanished mid-upload, or the record
        // was deleted without cleaning up the staging bytes first.
        let stray = Uuid::new_v4();
        objects.create_upload(ObjectKey::new(stray)).await.unwrap();

        let report = reconcile(&store, &objects, &collection).await.unwrap();
        assert!(report.broken.is_empty());
        assert_eq!(report.orphaned.len(), 1);
        assert_eq!(report.orphaned[0].id, Some(stray));
        assert!(report.orphaned[0].is_staging);
    }

    /// A resumable-upload staging file for a genuinely still-`pending`
    /// record is expected, legitimate machinery — never reported as an
    /// orphan just because its own asset hasn't finished uploading yet.
    #[tokio::test]
    async fn a_staging_file_for_a_pending_record_is_not_orphaned() {
        use crate::objectstore::ResumableUploadStore as _;

        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let record = register_managed(
            &store,
            &open_policy(),
            &collection,
            None,
            "thumb",
            RegisterManagedRequest {
                media_type: None,
                title: None,
                description: None,
                roles: vec![],
                declared_size: 5,
                digest: compute_digest(b"hello"),
            },
        )
        .await
        .unwrap();
        objects
            .create_upload(ObjectKey::new(record.id))
            .await
            .unwrap();

        let report = reconcile(&store, &objects, &collection).await.unwrap();
        assert!(report.is_clean(), "{report:?}");
    }

    #[tokio::test]
    async fn a_remote_asset_is_never_checked_against_the_store() {
        use crate::asset::{register_remote, RegisterRemoteRequest};

        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        register_remote(
            &store,
            &open_policy(),
            &collection,
            None,
            "external",
            RegisterRemoteRequest {
                href: "https://example.test/x".to_string(),
                media_type: None,
                title: None,
                description: None,
                roles: vec![],
            },
        )
        .await
        .unwrap();

        // A remote asset has no object-store presence at all — reconcile
        // must not treat its absence from `objects` as broken.
        let report = reconcile(&store, &objects, &collection).await.unwrap();
        assert!(report.is_clean(), "{report:?}");
    }
}
