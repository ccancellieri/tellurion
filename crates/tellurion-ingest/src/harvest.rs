//! `harvest stac` (`#191`): walks a remote STAC API and replays its items
//! into already-declared local collections **through the canonical write
//! lane** — `WriteSink::apply_batch`, the identical path the HTTP batch
//! route and `postgis load` drive. Nothing here writes SQL, and nothing
//! here issues DDL: every obligation a normal write produces (the
//! transactional outbox row, the derived index, tile invalidation, the
//! change feed) is produced because the write really went through the same
//! sink, not because this command re-implemented any of it.
//!
//! ## Why a harvest is also the rebuild tool
//!
//! Because a harvest is a canonical-write replay, **Tellurion's own STAC
//! surface is a valid `--source`**. Pointing this command at a catalog
//! served by this very deployment re-applies every item through the write
//! lane, which regenerates the derived index against the *current* DDL and
//! the *current* mapping — the "derived-index rebuild tool" the outbox
//! design doc left as an open question, as an ordinary resumable
//! idempotent job rather than a bespoke reindexer. Idempotency is what
//! makes that safe: every item is a caller-supplied-id upsert, so
//! re-harvesting a page (after an interrupt, or on purpose) converges
//! rather than duplicating.
//!
//! ## What this command deliberately does not do
//!
//! - **It never creates a target.** A remote collection with no published
//!   local counterpart is refused by name, never auto-declared: `ingest`
//!   owns all DDL, and publishing a collection is an explicit operator act
//!   (`registry publish-collection`).
//! - **It never derives physical identity.** The target `CollectionDecl`
//!   must pin `table`/`geometry`/`pk` — the same fully-pinned shape
//!   `Router::effective_decl`'s own fast path requires — because this CLI
//!   writes through a driver directly and has no router, no catalog source
//!   and no descriptor cache to derive them from.
//! - **It never copies bytes.** A remote asset is href-only (virtual):
//!   counted and reported, never fetched, never rewritten. Adoption of
//!   harvested assets by the assets subsystem (`#93`) is a later slice.
//! - **It never invents a target's shape.** Item properties are projected
//!   onto what the target declares; everything dropped is reported per
//!   collection. See `stac::map_item`'s own doc for the two projection
//!   rules.
//!
//! ## Resolution and storage
//!
//! Targets resolve through the relational registry (`#42`/`#143`) — the
//! same `registry_tenants`/`registry_catalogs`/`registry_collections`
//! tables `registry publish-*` writes and the server reads. A deployment
//! that declares its collections in `config.yaml` alone has no such lookup
//! and is out of scope for this slice, by name rather than by silence.
//! Writes go to the single PostGIS storage named by
//! `--database-url-env`; the target's own `storage` id is *not* resolved to
//! a different backend here, since a CLI without a config file has no
//! storage map to resolve it against.
//!
//! ## Report
//!
//! stdout is NDJSON, the same convention `batch_apply` established: one
//! `mapping` line per resolved collection (the id-mapping report), then
//! that collection's per-item `applied`/`refused`/`unapplied` lines, then a
//! `collection_summary`, and finally one `harvest_summary`. Item lines
//! carry no collection field on purpose — collections are harvested
//! strictly in sequence, each fenced by its own `mapping` and
//! `collection_summary` line.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use tellurion_core::{
    stage_batch_feature, BatchItemOutcome, BatchOutcomeLine, CollectionDecl, DriverFactory,
    Error as CoreError, Mutation, Problem, RegistryReader, RequestedCrs, Sequence, StorageDecl,
    StorageDriver, TenantReader, WriteSink,
};
use tellurion_postgis::{PostgisDriverFactory, PostgisRegistryReader, PostgisTenantReader};

mod stac;

use stac::{RemoteCollection, StacFetch};

/// Statement timeout for the two registry readers, in milliseconds, and the
/// driver's own request timeout, in seconds — the same 60 seconds
/// `postgis load` already gives its `PostgisDriverFactory`. One number for
/// both so a harvest has exactly one "how long may the database take"
/// answer, not two that can drift.
const TIMEOUT_SECONDS: u64 = 60;

/// The problem `instance` label every harvest-side refusal carries, the way
/// the batch lane labels its own `"batch"`.
const HARVEST_INSTANCE: &str = "harvest";

pub struct HarvestArgs {
    pub source: String,
    pub tenant: String,
    pub catalog: String,
    pub collections: Vec<String>,
    pub map: Vec<String>,
    pub max_items: Option<u64>,
    pub bookmark: Option<PathBuf>,
    pub database_url_env: String,
    pub chunk_items: usize,
    pub strict: bool,
    pub dry_run: bool,
}

/// A validated harvest request: every refusal this command can make without
/// touching the network or the database, made here, before either.
#[derive(Debug, PartialEq, Eq)]
struct HarvestPlan {
    root: String,
    tenant: String,
    catalog: String,
    /// Remote collection ids the operator asked for, in the order given.
    /// Empty means "every collection the source advertises".
    requested: Vec<String>,
    /// `remote id -> local collection external id`, for the entries
    /// `--map` renames. An unlisted remote id maps onto itself — the
    /// identity mapping is what makes a self-source rebuild a rebuild
    /// rather than a fan-out.
    renames: BTreeMap<String, String>,
    max_items: Option<u64>,
    chunk_items: usize,
    strict: bool,
    dry_run: bool,
}

impl HarvestPlan {
    fn from_args(args: &HarvestArgs) -> anyhow::Result<Self> {
        let root = stac::normalize_root(&args.source)?;
        anyhow::ensure!(!args.tenant.is_empty(), "--tenant must not be empty");
        anyhow::ensure!(!args.catalog.is_empty(), "--catalog must not be empty");
        anyhow::ensure!(args.chunk_items > 0, "--chunk-items must be at least 1");
        anyhow::ensure!(
            args.max_items != Some(0),
            "--max-items must be at least 1; omit it to harvest every item"
        );

        let mut requested = Vec::with_capacity(args.collections.len());
        for id in &args.collections {
            anyhow::ensure!(!id.is_empty(), "--collections must not contain an empty id");
            anyhow::ensure!(
                !requested.contains(id),
                "--collections names '{id}' more than once"
            );
            requested.push(id.clone());
        }

        let mut renames = BTreeMap::new();
        for entry in &args.map {
            let (remote, local) = entry.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("--map takes 'remote-id=local-id' pairs (got '{entry}')")
            })?;
            anyhow::ensure!(
                !remote.is_empty() && !local.is_empty(),
                "--map '{entry}' has an empty side"
            );
            anyhow::ensure!(
                renames
                    .insert(remote.to_string(), local.to_string())
                    .is_none(),
                "--map remaps '{remote}' more than once"
            );
            anyhow::ensure!(
                requested.is_empty() || requested.iter().any(|id| id == remote),
                "--map names '{remote}', which --collections does not request"
            );
        }

        Ok(Self {
            root,
            tenant: args.tenant.clone(),
            catalog: args.catalog.clone(),
            requested,
            renames,
            max_items: args.max_items,
            chunk_items: args.chunk_items,
            strict: args.strict,
            dry_run: args.dry_run,
        })
    }

    /// The local collection external id a remote id harvests into.
    fn local_id<'a>(&'a self, remote: &'a str) -> &'a str {
        self.renames
            .get(remote)
            .map(String::as_str)
            .unwrap_or(remote)
    }

    /// Narrows what the source advertises to what was asked for, preserving
    /// the operator's own `--collections` order so the report reads in the
    /// order it was requested. A requested id the source does not advertise
    /// is refused by name — silently harvesting a subset of what was asked
    /// for is how a "successful" harvest hides a typo.
    fn select(&self, advertised: Vec<RemoteCollection>) -> anyhow::Result<Vec<RemoteCollection>> {
        if self.requested.is_empty() {
            return Ok(advertised);
        }
        let mut selected = Vec::with_capacity(self.requested.len());
        let mut missing = Vec::new();
        for id in &self.requested {
            match advertised.iter().find(|collection| &collection.id == id) {
                Some(collection) => selected.push(collection.clone()),
                None => missing.push(id.clone()),
            }
        }
        anyhow::ensure!(
            missing.is_empty(),
            "the source advertises no collection named {}",
            missing
                .iter()
                .map(|id| format!("'{id}'"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        Ok(selected)
    }
}

/// Where a harvest left off, per remote collection. Written after every
/// fully-applied page, so an interrupted harvest resumes at a page boundary
/// rather than restarting the collection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct CollectionBookmark {
    /// The URL to fetch next. `None` with `complete: false` means "not
    /// started"; `None` with `complete: true` means the last page carried
    /// no `rel=next`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next: Option<String>,
    /// Items pulled from the source across every run, counted at page
    /// boundaries only — see [`harvest_collection`]'s own doc for why a
    /// `--max-items`-truncated page contributes nothing to it.
    #[serde(default)]
    harvested: u64,
    #[serde(default)]
    complete: bool,
}

/// The resume file. It records the harvest's identity as well as its
/// position: replaying a bookmark against a different source, tenant or
/// catalog would resume at page tokens that mean nothing there, so the
/// mismatch is refused rather than "resumed".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Bookmark {
    source: String,
    tenant: String,
    catalog: String,
    #[serde(default)]
    collections: BTreeMap<String, CollectionBookmark>,
}

impl Bookmark {
    fn new(plan: &HarvestPlan) -> Self {
        Self {
            source: plan.root.clone(),
            tenant: plan.tenant.clone(),
            catalog: plan.catalog.clone(),
            collections: BTreeMap::new(),
        }
    }

    fn load(path: &Path) -> anyhow::Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents)
                .map(Some)
                .with_context(|| format!("parsing bookmark '{}'", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("reading '{}'", path.display())),
        }
    }

    fn checked(self, plan: &HarvestPlan) -> anyhow::Result<Self> {
        anyhow::ensure!(
            self.source == plan.root,
            "bookmark was written for source '{}', not '{}'",
            self.source,
            plan.root
        );
        anyhow::ensure!(
            self.tenant == plan.tenant && self.catalog == plan.catalog,
            "bookmark was written for tenant '{}' catalog '{}', not tenant '{}' catalog '{}'",
            self.tenant,
            self.catalog,
            plan.tenant,
            plan.catalog
        );
        Ok(self)
    }

    /// Writes through a sibling temp file and renames it into place, so an
    /// interrupt mid-write leaves the previous bookmark intact rather than
    /// a truncated one a later run would refuse to parse.
    fn store(&self, path: &Path) -> anyhow::Result<()> {
        let temp = path.with_extension("bookmark-tmp");
        let contents = serde_json::to_vec_pretty(self).context("serializing bookmark")?;
        std::fs::write(&temp, &contents)
            .with_context(|| format!("writing '{}'", temp.display()))?;
        std::fs::rename(&temp, path)
            .with_context(|| format!("renaming '{}' into place", temp.display()))
    }
}

/// One resolved harvest target: the remote collection, the local
/// declaration it replays into, and the property names that declaration
/// pins (`None` when it pins no schema).
struct HarvestTarget {
    remote: RemoteCollection,
    decl: CollectionDecl,
    declared: Option<BTreeSet<String>>,
}

/// Refuses a target this CLI cannot write to without deriving its physical
/// identity from a catalog it has no router to reach. See this module's own
/// doc ("It never derives physical identity").
fn ensure_pinned(decl: &CollectionDecl) -> anyhow::Result<()> {
    let missing: Vec<&str> = [
        ("table", decl.table.is_some()),
        ("geometry", decl.geometry.is_some()),
        ("pk", decl.pk.is_some()),
    ]
    .into_iter()
    .filter_map(|(name, present)| (!present).then_some(name))
    .collect();
    anyhow::ensure!(
        missing.is_empty(),
        "collection '{}' does not pin {}; a harvest writes through the driver directly and cannot \
         run the router's catalog derivation — pin them in the published declaration",
        decl.external_id(),
        missing.join("/")
    );
    Ok(())
}

/// The declared property names a target pins, or `None` when it declares no
/// schema at all. An empty `properties` list is `None` too: "a schema that
/// declares no property" is not the same statement as "this collection has
/// no writable property", and reading it as the latter would drop every
/// property of every harvested item.
fn declared_properties(decl: &CollectionDecl) -> Option<BTreeSet<String>> {
    let schema = decl.schema.as_ref()?;
    if schema.properties.is_empty() {
        return None;
    }
    Some(
        schema
            .properties
            .iter()
            .map(|property| property.name.clone())
            .collect(),
    )
}

/// What one collection's harvest produced. `harvested`/`next`/`complete`
/// are the bookmark's own view (page boundaries); `applied`/`refused`/
/// `unapplied` are this run's item outcomes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct CollectionHarvest {
    pages: u64,
    applied: u64,
    refused: u64,
    unapplied: u64,
    assets: u64,
    dropped_properties: BTreeSet<String>,
    bookmark: CollectionBookmark,
    aborted: bool,
}

#[derive(Serialize)]
struct MappingLine<'a> {
    #[serde(rename = "type")]
    type_: &'static str,
    remote_collection: &'a str,
    collection: &'a str,
    internal_id: &'a str,
    catalog: &'a str,
    tenant: &'a str,
    table: &'a str,
    items_url: &'a str,
}

#[derive(Serialize)]
struct CollectionSummaryLine<'a> {
    #[serde(rename = "type")]
    type_: &'static str,
    remote_collection: &'a str,
    collection: &'a str,
    pages: u64,
    applied: u64,
    refused: u64,
    unapplied: u64,
    harvested: u64,
    assets: u64,
    dropped_properties: Vec<&'a str>,
    next: Option<&'a str>,
    complete: bool,
    outbox_high_water: Option<u64>,
}

#[derive(Serialize)]
struct HarvestSummaryLine {
    #[serde(rename = "type")]
    type_: &'static str,
    collections: u64,
    applied: u64,
    refused: u64,
    unapplied: u64,
    complete: bool,
    dry_run: bool,
}

pub async fn run(args: HarvestArgs) -> anyhow::Result<()> {
    let plan = HarvestPlan::from_args(&args)?;
    let mut bookmark = match &args.bookmark {
        Some(path) => Bookmark::load(path)?
            .map(|bookmark| bookmark.checked(&plan))
            .transpose()?
            .unwrap_or_else(|| Bookmark::new(&plan)),
        None => Bookmark::new(&plan),
    };

    let database_url = crate::db::read_url(&args.database_url_env)?;
    let fetch = stac::CurlFetch;

    let collections_url = format!("{}/collections", plan.root);
    tracing::info!(url = %collections_url, "walking STAC collections");
    let advertised =
        stac::parse_collections(&fetch.get(&collections_url).await?, &collections_url)?;
    let selected = plan.select(advertised)?;

    let tenant_reader =
        PostgisTenantReader::connect(&database_url, TIMEOUT_SECONDS * 1_000).await?;
    let tenant = tenant_reader
        .tenant(&plan.tenant)
        .await?
        .ok_or_else(|| anyhow::anyhow!("registry_tenants has no tenant '{}'", plan.tenant))?;
    let registry = PostgisRegistryReader::connect(&database_url, TIMEOUT_SECONDS * 1_000).await?;
    let catalog = registry
        .catalog(&tenant.id, &plan.catalog)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "tenant '{}' has no catalog '{}' in registry_catalogs",
                plan.tenant,
                plan.catalog
            )
        })?;

    let stdout = std::io::stdout();
    let mut output = stdout.lock();

    let mut targets = Vec::with_capacity(selected.len());
    for remote in selected {
        let local_id = plan.local_id(&remote.id).to_string();
        let decl = registry
            .collection(&catalog.id, &local_id)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "catalog '{}' has no collection '{local_id}' in registry_collections; publish \
                     it first (`tellurion-ingest registry publish-collection`) — a harvest never \
                     creates a target",
                    plan.catalog
                )
            })?;
        ensure_pinned(&decl)?;
        let items_url = stac::items_url(&plan.root, &remote)?;
        print_line(
            &mut output,
            &MappingLine {
                type_: "mapping",
                remote_collection: &remote.id,
                collection: decl.external_id(),
                internal_id: &decl.id,
                catalog: catalog.external_id(),
                tenant: tenant.external_id(),
                table: decl.resolved_table(),
                items_url: &items_url,
            },
        )?;
        let declared = declared_properties(&decl);
        targets.push((
            HarvestTarget {
                remote,
                decl,
                declared,
            },
            items_url,
        ));
    }

    if plan.dry_run {
        print_line(
            &mut output,
            &HarvestSummaryLine {
                type_: "harvest_summary",
                collections: targets.len() as u64,
                applied: 0,
                refused: 0,
                unapplied: 0,
                complete: false,
                dry_run: true,
            },
        )?;
        return Ok(());
    }

    let factory = PostgisDriverFactory::new(TIMEOUT_SECONDS);
    let storage = StorageDecl {
        id: "stac-harvest".to_string(),
        driver: "postgis".to_string(),
        url_env: args.database_url_env.clone(),
        pool_size: None,
    };
    let driver: std::sync::Arc<dyn StorageDriver> = factory.build(&storage)?;
    let sink = driver
        .write_sink()
        .ok_or_else(|| anyhow::anyhow!("postgis storage does not advertise a write sink"))?;
    let outbox = driver
        .outbox_source()
        .ok_or_else(|| anyhow::anyhow!("postgis storage does not advertise an outbox source"))?;

    let mut applied = 0u64;
    let mut refused = 0u64;
    let mut unapplied = 0u64;
    let mut complete = true;
    let mut aborted = false;

    for (target, items_url) in &targets {
        let resumed = bookmark
            .collections
            .get(&target.remote.id)
            .cloned()
            .unwrap_or_default();
        let harvest = {
            let remote_id = target.remote.id.clone();
            let bookmark_path = args.bookmark.clone();
            let bookmark = &mut bookmark;
            let mut page_committed = move |committed: &CollectionBookmark| -> anyhow::Result<()> {
                bookmark
                    .collections
                    .insert(remote_id.clone(), committed.clone());
                match &bookmark_path {
                    Some(path) => bookmark.store(path),
                    None => Ok(()),
                }
            };
            harvest_collection(
                &fetch,
                sink.as_ref(),
                target,
                items_url,
                &resumed,
                &plan,
                &mut output,
                &mut page_committed,
            )
            .await?
        };

        applied += harvest.applied;
        refused += harvest.refused;
        unapplied += harvest.unapplied;
        complete = complete && harvest.bookmark.complete;
        aborted = aborted || harvest.aborted;

        let outbox_high_water = match outbox.primary_high_water(&target.decl).await {
            Ok(Sequence(sequence)) => Some(sequence),
            Err(error) => {
                tracing::warn!(%error, collection = %target.decl.external_id(), "outbox high-water read failed");
                None
            }
        };
        print_line(
            &mut output,
            &CollectionSummaryLine {
                type_: "collection_summary",
                remote_collection: &target.remote.id,
                collection: target.decl.external_id(),
                pages: harvest.pages,
                applied: harvest.applied,
                refused: harvest.refused,
                unapplied: harvest.unapplied,
                harvested: harvest.bookmark.harvested,
                assets: harvest.assets,
                dropped_properties: harvest
                    .dropped_properties
                    .iter()
                    .map(String::as_str)
                    .collect(),
                next: harvest.bookmark.next.as_deref(),
                complete: harvest.bookmark.complete,
                outbox_high_water,
            },
        )?;

        if harvest.aborted {
            break;
        }
    }

    print_line(
        &mut output,
        &HarvestSummaryLine {
            type_: "harvest_summary",
            collections: targets.len() as u64,
            applied,
            refused,
            unapplied,
            complete: complete && !aborted,
            dry_run: false,
        },
    )?;
    anyhow::ensure!(
        !aborted,
        "harvest stopped at the first refused item (--strict); the bookmark still points at the \
         page that carried it"
    );
    Ok(())
}

/// Walks one collection's items from `start_url` (or the bookmark's own
/// `next`), applying every page through `sink` before advancing the
/// bookmark. `page_committed` is called with the bookmark's new value after
/// each fully-applied page — never after a page whose apply refused under
/// `--strict`, so a re-run retries exactly that page.
///
/// Under `--strict`, the items *after* the refusal on that page are never
/// mapped and never reported: unlike the batch lane's own strict abort —
/// which owes its caller a per-item verdict for a request body it already
/// accepted — a harvest owes nothing for pages it can simply fetch again.
/// The whole page is retried, which is why the bookmark must not advance
/// past it. Items of that page already staged before the refusal are still
/// applied (the same order the batch lane's own staging abort produces),
/// and re-applying them on the retry converges.
///
/// `--max-items` is a **cumulative** cap over the bookmark's own
/// `harvested` count, and it only ever counts *whole* pages: a page the cap
/// truncates leaves `harvested` at the boundary before it and parks `next`
/// on that same page, so resuming re-fetches it rather than skipping its
/// tail. A resume under an unchanged cap therefore has nothing left to do —
/// raising `--max-items` (or dropping it) is what continues the harvest.
/// Re-applying a page this way is safe precisely because every item is an
/// idempotent caller-supplied-id upsert.
#[allow(clippy::too_many_arguments)]
async fn harvest_collection(
    fetch: &dyn StacFetch,
    sink: &dyn WriteSink,
    target: &HarvestTarget,
    start_url: &str,
    resumed: &CollectionBookmark,
    plan: &HarvestPlan,
    output: &mut impl Write,
    page_committed: &mut impl FnMut(&CollectionBookmark) -> anyhow::Result<()>,
) -> anyhow::Result<CollectionHarvest> {
    let mut harvest = CollectionHarvest {
        bookmark: resumed.clone(),
        ..CollectionHarvest::default()
    };
    if resumed.complete {
        return Ok(harvest);
    }
    let mut url = Some(
        resumed
            .next
            .clone()
            .unwrap_or_else(|| start_url.to_string()),
    );
    let mut index = 0u64;

    while let Some(current) = url.take() {
        if plan
            .max_items
            .is_some_and(|max| harvest.bookmark.harvested >= max)
        {
            harvest.bookmark.next = Some(current);
            harvest.bookmark.complete = false;
            break;
        }

        tracing::info!(url = %current, collection = %target.decl.external_id(), "harvesting page");
        let body = fetch.get(&current).await?;
        let page = stac::parse_items_page(&body, &current)?;
        harvest.pages += 1;

        let page_start = harvest.bookmark.harvested;
        let mut harvested_here = 0u64;
        let mut truncated = false;
        let mut staged: Vec<(u64, Mutation)> = Vec::with_capacity(page.features.len());
        let mut lines: Vec<(u64, BatchOutcomeLine)> = Vec::new();

        for feature in &page.features {
            if plan
                .max_items
                .is_some_and(|max| page_start + harvested_here >= max)
            {
                truncated = true;
                break;
            }
            harvested_here += 1;
            let position = index;
            index += 1;
            match stac::map_item(feature, target.declared.as_ref()) {
                Ok(mapped) => {
                    harvest.assets += mapped.assets;
                    harvest
                        .dropped_properties
                        .extend(mapped.dropped_properties.iter().cloned());
                    match stage_batch_feature(mapped.feature, &target.decl) {
                        Ok(mutation) => staged.push((position, mutation)),
                        Err((id, error)) => {
                            harvest.refused += 1;
                            lines.push((
                                position,
                                BatchOutcomeLine::Refused {
                                    index: position,
                                    id,
                                    problem: Problem::from_core_error(&error, HARVEST_INSTANCE),
                                },
                            ));
                            if plan.strict {
                                harvest.aborted = true;
                                break;
                            }
                        }
                    }
                }
                Err(reason) => {
                    harvest.refused += 1;
                    lines.push((
                        position,
                        BatchOutcomeLine::Refused {
                            index: position,
                            id: feature
                                .get("id")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                            problem: Problem::from_core_error(
                                &CoreError::Invalid(reason),
                                HARVEST_INSTANCE,
                            ),
                        },
                    ));
                    if plan.strict {
                        harvest.aborted = true;
                        break;
                    }
                }
            }
        }

        let apply = apply_staged(
            sink,
            &target.decl,
            staged,
            plan.chunk_items,
            plan.strict,
            &mut harvest,
            &mut lines,
        )
        .await;
        lines.sort_by_key(|(position, _)| *position);
        for (_, line) in &lines {
            print_line(output, line)?;
        }
        apply?;

        if harvest.aborted {
            // The bookmark stays where it was: this page must be retried.
            harvest.bookmark.next = Some(current);
            harvest.bookmark.complete = false;
            break;
        }

        if truncated {
            harvest.bookmark.next = Some(current);
            harvest.bookmark.complete = false;
        } else {
            harvest.bookmark.harvested = page_start + harvested_here;
            harvest.bookmark.next = page.next.clone();
            harvest.bookmark.complete = page.next.is_none();
            url = page.next;
        }
        page_committed(&harvest.bookmark)?;
    }

    Ok(harvest)
}

/// Applies one page's staged mutations in `chunk_items`-sized transactions.
/// A chunk that fails outright reports every mutation it carried — plus
/// every later chunk's — as `unapplied` and returns the error: a harvest
/// that cannot write is not a harvest that should advance its bookmark.
async fn apply_staged(
    sink: &dyn WriteSink,
    decl: &CollectionDecl,
    staged: Vec<(u64, Mutation)>,
    chunk_items: usize,
    strict: bool,
    harvest: &mut CollectionHarvest,
    lines: &mut Vec<(u64, BatchOutcomeLine)>,
) -> anyhow::Result<()> {
    let mut remaining = staged.into_iter().peekable();
    let mut stop = false;
    while remaining.peek().is_some() {
        let chunk: Vec<(u64, Mutation)> = remaining.by_ref().take(chunk_items).collect();
        if stop {
            for (position, mutation) in chunk {
                harvest.unapplied += 1;
                lines.push((
                    position,
                    BatchOutcomeLine::Unapplied {
                        index: position,
                        id: Some(mutation.feature_id),
                    },
                ));
            }
            continue;
        }
        let mutations: Vec<Mutation> = chunk.iter().map(|(_, mutation)| mutation.clone()).collect();
        let results = match sink
            .apply_batch(decl, mutations, RequestedCrs::Omitted, strict)
            .await
        {
            Ok(results) => results,
            Err(error) => {
                for (position, mutation) in chunk {
                    harvest.unapplied += 1;
                    lines.push((
                        position,
                        BatchOutcomeLine::Unapplied {
                            index: position,
                            id: Some(mutation.feature_id),
                        },
                    ));
                }
                for (position, mutation) in remaining {
                    harvest.unapplied += 1;
                    lines.push((
                        position,
                        BatchOutcomeLine::Unapplied {
                            index: position,
                            id: Some(mutation.feature_id),
                        },
                    ));
                }
                harvest.aborted = true;
                return Err(anyhow::anyhow!(
                    "applying a harvested chunk into '{}' failed: {error}",
                    decl.external_id()
                ));
            }
        };
        for ((position, _), result) in chunk.iter().zip(&results) {
            match &result.outcome {
                BatchItemOutcome::Applied(Sequence(sequence)) => {
                    harvest.applied += 1;
                    lines.push((
                        *position,
                        BatchOutcomeLine::Applied {
                            index: *position,
                            id: result.feature_id.clone(),
                            sequence: *sequence,
                        },
                    ));
                }
                BatchItemOutcome::Refused(error) => {
                    harvest.refused += 1;
                    lines.push((
                        *position,
                        BatchOutcomeLine::Refused {
                            index: *position,
                            id: Some(result.feature_id.clone()),
                            problem: Problem::from_core_error(error, HARVEST_INSTANCE),
                        },
                    ));
                    if strict {
                        harvest.aborted = true;
                        stop = true;
                    }
                }
            }
        }
        for (position, mutation) in chunk.into_iter().skip(results.len()) {
            harvest.unapplied += 1;
            lines.push((
                position,
                BatchOutcomeLine::Unapplied {
                    index: position,
                    id: Some(mutation.feature_id),
                },
            ));
        }
    }
    Ok(())
}

fn print_line(output: &mut impl Write, value: &impl Serialize) -> anyhow::Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    output.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tellurion_core::{
        BatchItemResult, IdType, PropertyDecl, PropertyType, Result as CoreResult, RoutingDecl,
        SchemaDecl, SearchConf, SettingsDecl, StyleConf, TilesConf, VisibilityDecl,
    };

    use super::*;

    fn args(source: &str) -> HarvestArgs {
        HarvestArgs {
            source: source.to_string(),
            tenant: "acme".to_string(),
            catalog: "default".to_string(),
            collections: Vec::new(),
            map: Vec::new(),
            max_items: None,
            bookmark: None,
            database_url_env: "DATABASE_URL".to_string(),
            chunk_items: 2,
            strict: false,
            dry_run: false,
        }
    }

    fn plan(source: &str) -> HarvestPlan {
        HarvestPlan::from_args(&args(source)).expect("valid plan")
    }

    fn collection_decl(schema: Option<SchemaDecl>) -> CollectionDecl {
        CollectionDecl {
            id: "items-internal".to_string(),
            kind: tellurion_core::CollectionKind::Vector,
            external_id: Some("items".to_string()),
            catalog: "default".to_string(),
            storage: "main".to_string(),
            routing: RoutingDecl::default(),
            table: Some("items".to_string()),
            geometry: Some("geom".to_string()),
            pk: Some("id".to_string()),
            id_type: IdType::Text,
            datetime: None,
            modified_column: None,
            row_estimate: None,
            srid: None,
            projection: None,
            geometry_profile: None,
            tiles: TilesConf::default(),
            geometry_variants: Vec::new(),
            style: StyleConf::default(),
            places3d: None,
            schema,
            search: SearchConf::default(),
            tile_invalidation: false,
            settings: SettingsDecl::default(),
            attribute_columns: None,
            tile_properties: Vec::new(),
            visibility: VisibilityDecl::default(),
            object_store: None,
            stac_metadata: false,
            stac_item_assets: false,
        }
    }

    fn target(schema: Option<SchemaDecl>) -> HarvestTarget {
        let decl = collection_decl(schema);
        let declared = declared_properties(&decl);
        HarvestTarget {
            remote: RemoteCollection {
                id: "remote-items".to_string(),
                items_href: None,
            },
            decl,
            declared,
        }
    }

    struct FakeFetch {
        pages: BTreeMap<String, String>,
        fetched: Mutex<Vec<String>>,
    }

    impl FakeFetch {
        fn new(pages: &[(&str, serde_json::Value)]) -> Self {
            Self {
                pages: pages
                    .iter()
                    .map(|(url, body)| ((*url).to_string(), body.to_string()))
                    .collect(),
                fetched: Mutex::new(Vec::new()),
            }
        }

        fn fetched(&self) -> Vec<String> {
            self.fetched.lock().expect("lock is not poisoned").clone()
        }
    }

    #[async_trait::async_trait]
    impl StacFetch for FakeFetch {
        async fn get(&self, url: &str) -> anyhow::Result<String> {
            self.fetched
                .lock()
                .expect("lock is not poisoned")
                .push(url.to_string());
            self.pages
                .get(url)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no fake page for '{url}'"))
        }
    }

    #[derive(Default)]
    struct RecordingSink {
        applied: Mutex<Vec<String>>,
        /// Feature ids this sink refuses, to exercise the refusal lane.
        refuse: BTreeSet<String>,
    }

    #[async_trait::async_trait]
    impl WriteSink for RecordingSink {
        async fn apply(
            &self,
            _collection: &CollectionDecl,
            _mutation: Mutation,
        ) -> CoreResult<Sequence> {
            unreachable!("a harvest always writes through apply_batch")
        }

        async fn apply_batch(
            &self,
            _collection: &CollectionDecl,
            mutations: Vec<Mutation>,
            _requested_crs: RequestedCrs,
            strict: bool,
        ) -> CoreResult<Vec<BatchItemResult>> {
            let mut results = Vec::new();
            for mutation in mutations {
                let refused = self.refuse.contains(&mutation.feature_id);
                if !refused {
                    self.applied
                        .lock()
                        .expect("lock is not poisoned")
                        .push(mutation.feature_id.clone());
                }
                let outcome = if refused {
                    BatchItemOutcome::Refused(CoreError::Invalid("refused by fake".to_string()))
                } else {
                    BatchItemOutcome::Applied(Sequence(1))
                };
                results.push(BatchItemResult {
                    feature_id: mutation.feature_id,
                    outcome,
                });
                if refused && strict {
                    break;
                }
            }
            Ok(results)
        }
    }

    fn item(id: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "Feature",
            "id": id,
            "geometry": {"type": "Point", "coordinates": [1.0, 2.0]},
            "properties": {"datetime": "2026-01-01T00:00:00Z"}
        })
    }

    fn page(items: &[serde_json::Value], next: Option<&str>) -> serde_json::Value {
        let links = match next {
            Some(next) => serde_json::json!([{"rel": "next", "href": next}]),
            None => serde_json::json!([]),
        };
        serde_json::json!({
            "type": "FeatureCollection",
            "features": items,
            "links": links
        })
    }

    async fn walk(
        fetch: &FakeFetch,
        sink: &dyn WriteSink,
        target: &HarvestTarget,
        plan: &HarvestPlan,
        resumed: &CollectionBookmark,
    ) -> (
        anyhow::Result<CollectionHarvest>,
        Vec<serde_json::Value>,
        Vec<CollectionBookmark>,
    ) {
        let mut output = Vec::new();
        let mut committed = Vec::new();
        let result = harvest_collection(
            fetch,
            sink,
            target,
            "https://example.test/stac/collections/remote-items/items",
            resumed,
            plan,
            &mut output,
            &mut |bookmark: &CollectionBookmark| {
                committed.push(bookmark.clone());
                Ok(())
            },
        )
        .await;
        let lines = String::from_utf8(output)
            .expect("stdout is UTF-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("every line is NDJSON"))
            .collect();
        (result, lines, committed)
    }

    #[tokio::test]
    async fn walks_every_page_and_commits_a_bookmark_per_page() {
        let first = "https://example.test/stac/collections/remote-items/items";
        let second = "https://example.test/stac/collections/remote-items/items?token=2";
        let fetch = FakeFetch::new(&[
            (first, page(&[item("a"), item("b")], Some(second))),
            (second, page(&[item("c")], None)),
        ]);
        let sink = RecordingSink::default();
        let (harvest, lines, committed) = walk(
            &fetch,
            &sink,
            &target(None),
            &plan("https://example.test/stac"),
            &CollectionBookmark::default(),
        )
        .await;

        let harvest = harvest.expect("harvest succeeds");
        assert_eq!(harvest.applied, 3);
        assert_eq!(harvest.refused, 0);
        assert_eq!(harvest.pages, 2);
        assert!(harvest.bookmark.complete);
        assert_eq!(harvest.bookmark.harvested, 3);
        assert_eq!(harvest.bookmark.next, None);
        assert_eq!(fetch.fetched(), vec![first.to_string(), second.to_string()]);
        assert_eq!(
            *sink.applied.lock().expect("lock is not poisoned"),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().all(|line| line["type"] == "applied"));
        assert_eq!(
            committed,
            vec![
                CollectionBookmark {
                    next: Some(second.to_string()),
                    harvested: 2,
                    complete: false,
                },
                CollectionBookmark {
                    next: None,
                    harvested: 3,
                    complete: true,
                },
            ]
        );
    }

    #[tokio::test]
    async fn resumes_from_the_bookmark_and_skips_a_completed_collection() {
        let first = "https://example.test/stac/collections/remote-items/items";
        let second = "https://example.test/stac/collections/remote-items/items?token=2";
        let fetch = FakeFetch::new(&[(second, page(&[item("c")], None))]);
        let sink = RecordingSink::default();
        let resumed = CollectionBookmark {
            next: Some(second.to_string()),
            harvested: 2,
            complete: false,
        };
        let (harvest, _, _) = walk(
            &fetch,
            &sink,
            &target(None),
            &plan("https://example.test/stac"),
            &resumed,
        )
        .await;
        let harvest = harvest.expect("harvest succeeds");
        assert_eq!(harvest.applied, 1);
        assert_eq!(harvest.bookmark.harvested, 3);
        assert_eq!(fetch.fetched(), vec![second.to_string()]);
        assert!(!fetch.fetched().contains(&first.to_string()));

        // A completed collection is not re-walked at all.
        let fetch = FakeFetch::new(&[]);
        let complete = CollectionBookmark {
            next: None,
            harvested: 3,
            complete: true,
        };
        let (harvest, lines, committed) = walk(
            &fetch,
            &sink,
            &target(None),
            &plan("https://example.test/stac"),
            &complete,
        )
        .await;
        assert_eq!(harvest.expect("harvest succeeds").pages, 0);
        assert!(lines.is_empty());
        assert!(committed.is_empty());
        assert!(fetch.fetched().is_empty());
    }

    #[tokio::test]
    async fn max_items_parks_the_bookmark_on_the_truncated_page() {
        let first = "https://example.test/stac/collections/remote-items/items";
        let second = "https://example.test/stac/collections/remote-items/items?token=2";
        let fetch = FakeFetch::new(&[
            (
                first,
                page(&[item("a"), item("b"), item("c")], Some(second)),
            ),
            (second, page(&[item("d")], None)),
        ]);
        let sink = RecordingSink::default();
        let mut plan = plan("https://example.test/stac");
        plan.max_items = Some(2);
        let (harvest, _, committed) = walk(
            &fetch,
            &sink,
            &target(None),
            &plan,
            &CollectionBookmark::default(),
        )
        .await;

        let harvest = harvest.expect("harvest succeeds");
        assert_eq!(harvest.applied, 2);
        assert!(!harvest.bookmark.complete);
        // Nothing is committed past the page boundary before the truncated
        // page, so a resume re-fetches that page rather than skipping its tail.
        assert_eq!(harvest.bookmark.harvested, 0);
        assert_eq!(harvest.bookmark.next.as_deref(), Some(first));
        assert_eq!(fetch.fetched(), vec![first.to_string()]);
        assert_eq!(
            committed,
            vec![CollectionBookmark {
                next: Some(first.to_string()),
                harvested: 0,
                complete: false,
            }]
        );
    }

    #[tokio::test]
    async fn an_unmappable_item_is_refused_by_name_without_stopping_the_page() {
        let first = "https://example.test/stac/collections/remote-items/items";
        let broken = serde_json::json!({
            "type": "Feature",
            "id": "broken",
            "geometry": null,
            "properties": {},
            "assets": {"data": {"type": "image/tiff"}}
        });
        let fetch = FakeFetch::new(&[(first, page(&[item("a"), broken, item("b")], None))]);
        let sink = RecordingSink::default();
        let (harvest, lines, _) = walk(
            &fetch,
            &sink,
            &target(None),
            &plan("https://example.test/stac"),
            &CollectionBookmark::default(),
        )
        .await;

        let harvest = harvest.expect("harvest succeeds");
        assert_eq!(harvest.applied, 2);
        assert_eq!(harvest.refused, 1);
        assert!(harvest.bookmark.complete);
        assert_eq!(lines[1]["type"], "refused");
        assert_eq!(lines[1]["id"], "broken");
        assert!(
            lines[1]["problem"]["detail"]
                .as_str()
                .expect("a refusal carries a detail")
                .contains("never mints one"),
            "{:?}",
            lines[1]
        );
    }

    #[tokio::test]
    async fn strict_stops_at_the_first_refusal_and_leaves_the_bookmark_on_that_page() {
        let first = "https://example.test/stac/collections/remote-items/items";
        let second = "https://example.test/stac/collections/remote-items/items?token=2";
        let fetch = FakeFetch::new(&[
            (
                first,
                page(&[item("a"), item("b"), item("c")], Some(second)),
            ),
            (second, page(&[item("d")], None)),
        ]);
        let sink = RecordingSink {
            refuse: BTreeSet::from(["b".to_string()]),
            ..RecordingSink::default()
        };
        let mut plan = plan("https://example.test/stac");
        plan.strict = true;
        plan.chunk_items = 3;
        let (harvest, lines, committed) = walk(
            &fetch,
            &sink,
            &target(None),
            &plan,
            &CollectionBookmark::default(),
        )
        .await;

        let harvest = harvest.expect("harvest succeeds");
        assert!(harvest.aborted);
        assert_eq!(harvest.applied, 1);
        assert_eq!(harvest.refused, 1);
        assert_eq!(harvest.unapplied, 1);
        assert_eq!(harvest.bookmark.next.as_deref(), Some(first));
        assert!(!harvest.bookmark.complete);
        assert!(committed.is_empty(), "a refused page never commits");
        assert_eq!(
            lines
                .iter()
                .map(|line| line["type"].as_str().expect("a line always has a type"))
                .collect::<Vec<_>>(),
            vec!["applied", "refused", "unapplied"]
        );
    }

    #[tokio::test]
    async fn a_pinned_schema_projects_the_properties_that_reach_the_sink() {
        let first = "https://example.test/stac/collections/remote-items/items";
        let rich = serde_json::json!({
            "type": "Feature",
            "id": "a",
            "geometry": {"type": "Point", "coordinates": [1.0, 2.0]},
            "properties": {"datetime": "2026-01-01T00:00:00Z", "undeclared": 7}
        });
        let fetch = FakeFetch::new(&[(first, page(&[rich], None))]);
        let sink = RecordingSink::default();
        let schema = SchemaDecl {
            properties: vec![PropertyDecl {
                name: "datetime".to_string(),
                type_: PropertyType::String,
                required: false,
            }],
            additional_properties: true,
        };
        let (harvest, _, _) = walk(
            &fetch,
            &sink,
            &target(Some(schema)),
            &plan("https://example.test/stac"),
            &CollectionBookmark::default(),
        )
        .await;
        let harvest = harvest.expect("harvest succeeds");
        assert_eq!(harvest.applied, 1);
        assert_eq!(
            harvest.dropped_properties,
            BTreeSet::from(["undeclared".to_string()])
        );
    }

    #[test]
    fn plan_refuses_every_shape_it_cannot_harvest() {
        let mut bad_source = args("ftp://example.test");
        bad_source.source = "ftp://example.test".to_string();
        let error = HarvestPlan::from_args(&bad_source).unwrap_err().to_string();
        assert!(error.contains("http(s) STAC API root"), "{error}");

        let mut zero = args("https://example.test");
        zero.max_items = Some(0);
        let error = HarvestPlan::from_args(&zero).unwrap_err().to_string();
        assert!(error.contains("--max-items must be at least 1"), "{error}");

        let mut chunkless = args("https://example.test");
        chunkless.chunk_items = 0;
        let error = HarvestPlan::from_args(&chunkless).unwrap_err().to_string();
        assert!(error.contains("--chunk-items"), "{error}");

        let mut duplicated = args("https://example.test");
        duplicated.collections = vec!["a".to_string(), "a".to_string()];
        let error = HarvestPlan::from_args(&duplicated).unwrap_err().to_string();
        assert!(error.contains("more than once"), "{error}");

        let mut unpaired = args("https://example.test");
        unpaired.map = vec!["a".to_string()];
        let error = HarvestPlan::from_args(&unpaired).unwrap_err().to_string();
        assert!(error.contains("'remote-id=local-id' pairs"), "{error}");

        let mut unrequested = args("https://example.test");
        unrequested.collections = vec!["a".to_string()];
        unrequested.map = vec!["b=c".to_string()];
        let error = HarvestPlan::from_args(&unrequested)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("which --collections does not request"),
            "{error}"
        );
    }

    #[test]
    fn selection_preserves_the_requested_order_and_names_a_missing_collection() {
        let advertised = vec![
            RemoteCollection {
                id: "a".to_string(),
                items_href: None,
            },
            RemoteCollection {
                id: "b".to_string(),
                items_href: None,
            },
        ];
        let mut plan = plan("https://example.test");
        plan.requested = vec!["b".to_string(), "a".to_string()];
        let selected = plan
            .select(advertised.clone())
            .expect("both are advertised");
        assert_eq!(
            selected.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );

        plan.requested = vec!["a".to_string(), "missing".to_string()];
        let error = plan.select(advertised).unwrap_err().to_string();
        assert!(
            error.contains("advertises no collection named 'missing'"),
            "{error}"
        );
    }

    #[test]
    fn local_id_is_the_identity_mapping_unless_map_renames_it() {
        let mut renamed = args("https://example.test");
        renamed.map = vec!["remote=local".to_string()];
        let plan = HarvestPlan::from_args(&renamed).expect("valid plan");
        assert_eq!(plan.local_id("remote"), "local");
        assert_eq!(plan.local_id("other"), "other");
    }

    #[test]
    fn an_unpinned_target_is_refused_by_name() {
        let mut decl = collection_decl(None);
        decl.table = None;
        decl.pk = None;
        let error = ensure_pinned(&decl).unwrap_err().to_string();
        assert!(error.contains("does not pin table/pk"), "{error}");
        assert!(ensure_pinned(&collection_decl(None)).is_ok());
    }

    #[test]
    fn an_empty_schema_declaration_is_not_read_as_zero_writable_properties() {
        assert_eq!(declared_properties(&collection_decl(None)), None);
        assert_eq!(
            declared_properties(&collection_decl(Some(SchemaDecl::default()))),
            None
        );
        let schema = SchemaDecl {
            properties: vec![PropertyDecl {
                name: "datetime".to_string(),
                type_: PropertyType::String,
                required: false,
            }],
            additional_properties: false,
        };
        assert_eq!(
            declared_properties(&collection_decl(Some(schema))),
            Some(BTreeSet::from(["datetime".to_string()]))
        );
    }

    #[test]
    fn a_bookmark_written_for_another_harvest_is_refused_rather_than_resumed() {
        let plan = plan("https://example.test/stac");
        let bookmark = Bookmark {
            source: "https://elsewhere.test/stac".to_string(),
            tenant: "acme".to_string(),
            catalog: "default".to_string(),
            collections: BTreeMap::new(),
        };
        let error = bookmark.checked(&plan).unwrap_err().to_string();
        assert!(error.contains("written for source"), "{error}");

        let bookmark = Bookmark {
            source: "https://example.test/stac".to_string(),
            tenant: "other".to_string(),
            catalog: "default".to_string(),
            collections: BTreeMap::new(),
        };
        let error = bookmark.checked(&plan).unwrap_err().to_string();
        assert!(error.contains("written for tenant 'other'"), "{error}");

        let bookmark = Bookmark::new(&plan);
        assert!(bookmark.checked(&plan).is_ok());
    }

    #[test]
    fn a_bookmark_round_trips_through_the_file_it_is_stored_in() {
        let directory = tempfile::tempdir().expect("creates a temp directory");
        let path = directory.path().join("harvest.bookmark");
        assert_eq!(
            Bookmark::load(&path).expect("a missing file is not an error"),
            None
        );

        let mut bookmark = Bookmark::new(&plan("https://example.test/stac"));
        bookmark.collections.insert(
            "remote-items".to_string(),
            CollectionBookmark {
                next: Some("https://example.test/next".to_string()),
                harvested: 12,
                complete: false,
            },
        );
        bookmark.store(&path).expect("stores the bookmark");
        assert_eq!(
            Bookmark::load(&path).expect("reads it back"),
            Some(bookmark)
        );
    }

    /// Live-database test: proves the claim the whole design rests on —
    /// that a harvest is a *real* write, not a shortcut. The same page is
    /// harvested twice through the actual PostGIS `WriteSink`; the rows
    /// converge (idempotent caller-supplied-id upsert, which is what makes
    /// a self-source rebuild safe) and the transactional outbox carries an
    /// obligation per applied item, because the write genuinely went
    /// through the driver's own write lane. Skips gracefully unless
    /// `TELLURION_TEST_DATABASE_URL` is set.
    #[tokio::test]
    async fn harvests_through_the_real_write_lane_and_leaves_outbox_obligations() {
        const URL_ENV: &str = "TELLURION_TEST_DATABASE_URL";
        let Ok(url) = std::env::var(URL_ENV) else {
            eprintln!("skipping: {URL_ENV} not set");
            return;
        };
        let table = "tellurion_ingest_harvest_live";

        let client = crate::db::connect_url(&url)
            .await
            .expect("connect to test database");
        // `#272`: this fixture creates the very table `outbox::create_tables`
        // below now locks, so it takes the same lock — under the same name,
        // through `#138`'s harness. Left raw, it would race that call and
        // any live suite pointed at the same database.
        tellurion_postgis::test_harness::apply_fixture_ddl(
            &client,
            table,
            &format!(
                "DROP TABLE IF EXISTS \"{table}_outbox\"; DROP TABLE IF EXISTS \"{table}\";
                 CREATE TABLE \"{table}\" (
                     id text PRIMARY KEY,
                     geom geometry(Geometry, 4326),
                     datetime text
                 );"
            ),
        )
        .await
        .expect("provision the harvest target table");
        crate::outbox::create_tables(crate::outbox::CreateTablesArgs {
            table: table.to_string(),
            database_url_env: URL_ENV.to_string(),
            dry_run: false,
        })
        .await
        .expect("provision the outbox table");

        let driver = PostgisDriverFactory::new(TIMEOUT_SECONDS)
            .build(&StorageDecl {
                id: "harvest-live".to_string(),
                driver: "postgis".to_string(),
                url_env: URL_ENV.to_string(),
                pool_size: None,
            })
            .expect("builds the postgis driver");
        let sink = driver
            .write_sink()
            .expect("postgis advertises a write sink");

        let mut decl = collection_decl(None);
        decl.table = Some(table.to_string());
        decl.srid = Some(4326);
        let target = HarvestTarget {
            remote: RemoteCollection {
                id: "remote-items".to_string(),
                items_href: None,
            },
            decl,
            declared: None,
        };

        let first = "https://example.test/stac/collections/remote-items/items";
        let plan = plan("https://example.test/stac");
        for _ in 0..2 {
            let fetch = FakeFetch::new(&[(first, page(&[item("a"), item("b")], None))]);
            let (harvest, _, _) = walk(
                &fetch,
                sink.as_ref(),
                &target,
                &plan,
                &CollectionBookmark::default(),
            )
            .await;
            let harvest = harvest.expect("the harvest applies through PostGIS");
            assert_eq!(harvest.applied, 2);
            assert_eq!(harvest.refused, 0);
        }

        let rows: i64 = client
            .query_one(&format!("SELECT count(*) FROM \"{table}\""), &[])
            .await
            .expect("counts the harvested rows")
            .get(0);
        assert_eq!(rows, 2, "a re-harvest converges rather than duplicating");
        let obligations: i64 = client
            .query_one(
                &format!("SELECT count(*) FROM \"{table}_outbox\" WHERE kind = 'upsert'"),
                &[],
            )
            .await
            .expect("counts the outbox obligations")
            .get(0);
        assert_eq!(
            obligations, 4,
            "every applied item leaves its own obligation, re-harvest included"
        );
        let datetime: String = client
            .query_one(
                &format!("SELECT datetime FROM \"{table}\" WHERE id = 'a'"),
                &[],
            )
            .await
            .expect("reads the harvested property back")
            .get(0);
        assert_eq!(datetime, "2026-01-01T00:00:00Z");
    }
}
