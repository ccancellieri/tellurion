//! Per-item STAC metadata sidecar (`#202`, third slice of the sidecar line
//! of work): the capability a `StorageDriver` advertises
//! (`router::StorageDriver::stac_metadata_source`) so the STAC lane — and
//! ONLY the STAC lane — can enrich an Item with metadata that has no place
//! in the collection's own feature properties.
//!
//! ## Why a sidecar at all
//!
//! A STAC Item is the collection's GeoJSON feature enriched in place by
//! `tellurion-stac::mapping::to_stac_item`. An operator serving one table
//! over both OGC API Features and STAC has nowhere to put STAC-specific
//! per-item metadata (`eo:cloud_cover`, a per-scene `stac_extensions` list,
//! a corrected `datetime`) without adding columns to the primary table —
//! columns the Features lane would then serve too. This capability is that
//! place: a per-collection `"<table>_stac"` table, keyed by `feature_id`,
//! read on the STAC lane only.
//!
//! ## What this capability does NOT do
//!
//! Nothing writes it in this slice: the table is populated out-of-band by
//! ingest, exactly like `#201`'s geometry variants. Maintaining it on write
//! (an applier with a pluggable derivation over the existing outbox) is a
//! later slice, so this trait is read-only by construction — there is no
//! `apply` here to leave half-specified.
//!
//! ## Provisioning, not capability
//!
//! Advertising this says nothing about whether a given collection's
//! `"<table>_stac"` table exists: the server never does DDL (see
//! `tellurion-ingest::stac`), so a collection that declares
//! `stac_metadata: true` but was never provisioned gets a named
//! request-time refusal from the driver itself (PostGIS:
//! `StacTableMissing`), the identical treatment `OutboxTableMissing` /
//! `IndexTableMissing` / `AssetsTableMissing` already get — never a
//! capability check, and never a silent empty answer, which would be
//! indistinguishable from "this page's items simply have no sidecar rows".

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;

use crate::config::CollectionDecl;
use crate::error::Result;

/// Batched per-item STAC metadata reads over a collection's
/// `"<table>_stac"` sidecar.
///
/// [`stac_metadata`](Self::stac_metadata) takes the whole page's
/// `feature_ids` at once and answers with a `feature_id -> doc` map, so a
/// page of N items costs ONE extra round trip rather than N (PostGIS
/// compiles it to `feature_id = ANY($1)`). The map is deliberately sparse:
/// a `feature_id` with no sidecar row is simply absent from it — "this item
/// has no sidecar metadata" is the ordinary case, not an error, and it must
/// stay byte-for-byte indistinguishable from a collection that has no
/// sidecar at all.
///
/// An empty `feature_ids` slice MUST answer `Ok(<empty map>)` without
/// touching the backend at all: an empty page has nothing to enrich, and a
/// round trip for it would violate the "one extra round trip per page,
/// never one per item" budget in the other direction.
#[async_trait]
pub trait StacMetadataSource: Send + Sync {
    async fn stac_metadata(
        &self,
        collection: &CollectionDecl,
        feature_ids: &[String],
    ) -> Result<HashMap<String, Value>>;
}
