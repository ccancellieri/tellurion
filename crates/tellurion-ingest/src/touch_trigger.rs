//! Optional `modified_column` touch trigger (`#151`) — the maintenance half
//! of `#149`'s declaration.
//!
//! `CollectionDecl::modified_column` names a real backend column and gates
//! `req/optimistic-locking-timestamps` on it (`#107`, `#149`), but **nothing
//! in this workspace writes it**: the PostGIS write path
//! (`tellurion-postgis::write_sql`) never mentions the column, and neither
//! does any other lane. An operator who declares one is responsible for
//! bumping it on every write, and a column nobody bumps silently turns
//! `If-Unmodified-Since` into a guard that always passes — the failure `#151`
//! exists to close.
//!
//! This module provisions the standard `BEFORE INSERT OR UPDATE ... SET
//! <column> = now()` trigger so the declaration and its maintenance can land
//! together. It is **strictly opt-in**: a deployment that never runs
//! `tellurion-ingest locking install-touch-trigger` gets no trigger, no
//! altered column and no changed behaviour whatsoever — nothing in
//! `tellurion-server` learns anything because this command exists, exactly
//! the posture `variants.rs` states for its own backfill. There is no config
//! key, no setting and no default here to get wrong; the entire opt-in is
//! "an operator ran a command".
//!
//! A trigger is DDL, so it comes from here and only from here — the same
//! "ingest owns all DDL" rule `outbox.rs`/`index.rs`/`assets.rs`/
//! `processes.rs`/`geopackage.rs` already hold. The server would not create
//! one even if it wanted it to exist.
//!
//! ## PostGIS only, and the rest refused by name
//!
//! PostgreSQL is the only backend this command provisions for. GeoPackage
//! (SQLite) has triggers too, but a different dialect (`CREATE TRIGGER ...
//! BEGIN UPDATE ... END`, no `NEW` assignment — a SQLite `BEFORE UPDATE`
//! trigger cannot rewrite the row being written, it can only issue a second
//! `UPDATE`, which recurses into itself unless guarded) and different
//! semantics; `#151` says explicitly to keep it PostGIS-only until the demand
//! for the SQLite form is real. A collection on any other driver is refused
//! by name, naming the driver — never quietly skipped, and never
//! approximated with a dialect this command has not proved.
//!
//! Nothing about the *declaration* is PostGIS-only: a GeoPackage collection
//! may still declare a `modified_column` and still serve
//! `req/optimistic-locking-timestamps` off it. It just has to maintain it
//! some other way. This command narrows to the one backend whose trigger form
//! it actually implements, not to the one backend allowed to have the column.
//!
//! ## `BEFORE INSERT OR UPDATE`, not `BEFORE UPDATE`
//!
//! `#151`'s own sketch says `BEFORE UPDATE`. It is deliberately widened here,
//! because `PUT /collections/{cid}/items/{fid}` against a new id **creates**
//! the row (`WriteSink::apply`'s `MutationKind::Upsert`, the contract
//! `write_handlers.rs` documents). An `UPDATE`-only trigger leaves such a row
//! with whatever the insert supplied — `NULL` for a column with no default,
//! which `locking::parse_stored_timestamp` cannot read, which makes
//! `Last-Modified` absent and `If-Unmodified-Since` a silent no-op on exactly
//! the rows a write-heavy collection has most of. That is the same hole in a
//! narrower place, so the insert arm is part of the trigger rather than
//! something the operator has to remember to also put a column `DEFAULT` on.
//!
//! ## Unconditional, with no `WHEN (OLD.* IS DISTINCT FROM NEW.*)`
//!
//! Also deliberate, and this is where the trigger meets `#150`.
//!
//! `#150` made a satisfied precondition travel to the driver as an opaque
//! `locking::RowVersion` witness that the database re-verifies in the same
//! transaction as the write. PostGIS mints that witness from the row's
//! `xmin`, which **any** write to the row changes — including one that stores
//! byte-identical values. `#150` recorded the consequence honestly: on
//! PostGIS `If-Unmodified-Since` guards slightly more strictly than its
//! literal wording, because the witness detects any row change rather than
//! only a `modified_column` change.
//!
//! An unconditional touch trigger **closes that divergence** rather than
//! widening it: `now()` lands on every row version `xmin` also changes, so
//! the column and the witness answer the same question. A `WHEN (OLD.* IS
//! DISTINCT FROM NEW.*)` guard would do the opposite — an update writing
//! identical values would bump `xmin` and not the column, so the two guards
//! would disagree on precisely the case `#150` called out. The cheaper
//! trigger is the less correct one here.
//!
//! What the trigger does **not** do is change what the witness sees. It is a
//! `BEFORE` row trigger: it rewrites `NEW` before the tuple is written, so
//! the statement produces one row version, not two, and the `WHERE ... AND
//! xmin = $witness` predicate the conditional write compiles is evaluated
//! against the *old* tuple, before this trigger has run at all. Provisioning
//! it neither weakens nor spuriously trips `#150`'s guard.
//!
//! One knock-on worth stating out loud, since it reaches the OTHER Optimistic
//! Locking class: the declared column is served as an ordinary property, so it
//! is part of the representation `locking::compute_feature_etag` hashes. With
//! the trigger installed, a `PUT` that stores byte-identical values still
//! changes `modified` and therefore still changes the `ETag`. Without it, such
//! a `PUT` left the `ETag` untouched. That is a real behaviour change for a
//! deployment that installs this — and it moves the `ETag` towards what a
//! strong validator is supposed to mean, since the resource's representation
//! genuinely did change. It is also, again, the same answer the `xmin` witness
//! was already giving.
//!
//! ## `now()`, not `clock_timestamp()`
//!
//! `now()` is transaction-start time, so every row a transaction touches gets
//! the same stamp — the same granularity `xmin` has, one value per
//! transaction. `clock_timestamp()` would let two rows written by one
//! transaction disagree about when "the change" happened.
//!
//! Being transaction-start time, the stamp is not a total order of *commits*:
//! a long transaction that started earlier can commit later and store an
//! earlier value than a short one that already committed. No timestamp
//! column fixes that, and it is exactly why `#150`'s witness exists — the
//! timestamp is the client-facing validator, the witness is what actually
//! closes the check-to-apply window. This trigger makes the validator honest;
//! it does not make it sufficient on its own, and never claimed to.
//!
//! ## What changes, and for whom
//!
//! A Rust-side touch in the write lane would only ever fire for requests that
//! *reach* the write lane. A trigger fires for every writer of the table:
//!
//! - **`PUT`/`POST`/`DELETE`/`PATCH` through the write lane** —
//!   `modified_column` now moves on every landed write. Previously it moved
//!   only if the operator's own application moved it.
//! - **`tellurion-ingest postgis load` and `tellurion-ingest harvest stac`**
//!   — these go through `WriteSink::apply_batch`, so they write the data
//!   table and the trigger fires. A harvest that carried an upstream
//!   modification time in that column no longer preserves it: the column
//!   means "when this server's copy last changed", which is what Part 4
//!   compares an `If-Unmodified-Since` against, so `now()` is the correct
//!   value and the upstream one belongs in a column of its own.
//! - **`tellurion-ingest seed` / `load` (raw SQL and `ogr2ogr`)** — these
//!   bypass the write lane entirely and would be invisible to any Rust-side
//!   touch. With the trigger they stamp `now()` too. This is the difference
//!   `#151` exists for.
//! - **Anything else that writes the table** — a DBA's fix-up `UPDATE`, a
//!   bulk `COPY`, another application sharing the database. All stamped.
//!   A Rust-side touch could never have covered any of them.
//! - **The applier (`crate::applier::run_applier`)** — *unaffected*. It
//!   drains `"<table>_outbox"` into `"<table>_index"`; it does not write the
//!   data table, so this trigger never fires for it.
//!
//! ## A deployment that already maintains the column itself
//!
//! Application-side maintenance is **not detectable from the database**, and
//! this command does not pretend otherwise. What it can see, it checks:
//!
//! - a column that is `GENERATED ALWAYS` cannot be assigned by a trigger at
//!   all, so it is refused by name rather than left to fail at write time;
//! - any *other* row-level trigger on the same table whose function body
//!   mentions the declared column is refused by name, naming the trigger and
//!   its function. Two triggers assigning one column run in trigger-name
//!   order, which is not a thing an operator should have to reason about.
//!   `--allow-existing-trigger` is the explicit way through for a false
//!   positive (a body that merely reads the column, say), the same
//!   consent-flag shape `variants.rs` uses for its own detectable-but-
//!   arguable case;
//! - a function already occupying the derived name that this command did not
//!   create is refused by name rather than silently replaced — every function
//!   this module installs carries [`TOUCH_MARKER`] in its body, so ownership
//!   is a fact about the database rather than a convention.
//!
//! For the undetectable case — an application issuing `SET modified = ...` in
//! its own `UPDATE` — the trigger does not double-write and does not fight
//! it: a `BEFORE` trigger is the *last* writer of `NEW` before the tuple is
//! written, so the trigger's `now()` supersedes the application's value
//! within the same single row version. That is a change in which value wins,
//! not a conflict, and `now()` is the value Part 4's comparison wants. The
//! command says so on the way out rather than leaving an operator to discover
//! it.

use std::path::PathBuf;

use anyhow::Context;
use tellurion_core::config::PropertyType;
use tellurion_core::{AppConfig, CollectionDecl, StorageDecl};

/// Stamped into every trigger function body this module installs, so
/// "did this command create that function?" is answerable from `pg_proc`
/// rather than assumed from a name. See the module doc's "already maintains
/// the column itself" section.
pub(crate) const TOUCH_MARKER: &str = "tellurion:modified-column-touch";

/// The one driver whose touch-trigger dialect this command implements. See
/// the module doc for why GeoPackage is refused rather than approximated.
const SUPPORTED_DRIVER: &str = "postgis";

pub struct InstallArgs {
    /// Tellurion config YAML declaring the collection — read only, never
    /// written. Its `modified_column` is the source of truth for which
    /// column the trigger maintains, so this command can never provision a
    /// trigger for a column no collection declares.
    pub config: PathBuf,
    /// Internal id of the collection whose declared `modified_column` to
    /// maintain.
    pub collection: String,
    /// Consent to installing alongside another row-level trigger on the same
    /// table whose function body mentions the declared column. See the module
    /// doc.
    pub allow_existing_trigger: bool,
    /// Print the DDL without connecting to a database at all — the same
    /// escape hatch every `create-tables` command in this crate offers an
    /// operator with no direct CLI database access.
    pub dry_run: bool,
}

/// Everything one installation needs, resolved from config alone. Kept as a
/// plain value so the driver refusal and the SQL text are assertable with no
/// backend at all.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TouchPlan {
    /// Environment variable holding the Postgres connection string — the
    /// storage's own `url_env`, never a separate CLI flag: the collection
    /// already names its storage and the storage already names its
    /// connection, the same chain `variants materialize` follows.
    pub(crate) url_env: String,
    pub(crate) table: String,
    pub(crate) column: String,
}

impl TouchPlan {
    /// `"<table>_<column>_touch"` — the trigger function. Functions are
    /// schema-scoped, so the table has to be in the name; the column too,
    /// since a table may legitimately carry a touch trigger for more than one
    /// column.
    pub(crate) fn function_name(&self) -> String {
        format!("{}_{}_touch", self.table, self.column)
    }

    /// `"<table>_<column>_touch_trg"` — the trigger itself. Triggers are
    /// table-scoped in Postgres, so this only has to be unique per table, but
    /// it is derived the same way for the same reason `outbox.rs` derives
    /// `"<table>_outbox"`: an operator should be able to predict the name
    /// without reading this file.
    pub(crate) fn trigger_name(&self) -> String {
        format!("{}_{}_touch_trg", self.table, self.column)
    }
}

/// Whitelist-validates and double-quotes `name` for use as a SQL identifier —
/// this module's own local copy of the rule every other DDL module in this
/// crate hand-keeps (`assets.rs`, `index.rs`, `outbox.rs`, `variants.rs`);
/// see any of those for why it stays a local copy rather than a shared
/// helper. Rejects outright rather than transforming: a derived trigger name
/// that got mangled into something valid would be a trigger no rerun of this
/// command could ever find again.
fn quote_ident(name: &str) -> anyhow::Result<String> {
    let mut chars = name.chars();
    let first = chars
        .next()
        .filter(|c| c.is_ascii_alphabetic() || *c == '_');
    if first.is_none() || name.len() > 63 || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        anyhow::bail!(
            "'{name}' is not a valid Postgres identifier: only ASCII letters, digits, and '_' are allowed, it may not start with a digit, and it may not exceed 63 bytes"
        );
    }
    Ok(format!("\"{name}\""))
}

/// The trigger function plus the trigger, as one idempotent statement batch.
///
/// Both halves are `CREATE OR REPLACE` (PostgreSQL 14+ for the trigger form;
/// this workspace deploys `postgis/postgis:16-3.4`), so a rerun is a no-op
/// rather than an error — the same rerun-is-safe property every `CREATE TABLE
/// IF NOT EXISTS` in this crate has. `DROP TRIGGER IF EXISTS` followed by
/// `CREATE TRIGGER` would work on older servers but leaves a window in which
/// the table has no trigger at all, which is the one thing an idempotent
/// reinstall must not do.
///
/// See the module doc for why the timing is `BEFORE INSERT OR UPDATE`, why
/// there is no `WHEN` clause, and why the function is `now()` rather than
/// `clock_timestamp()`.
pub(crate) fn touch_trigger_sql(plan: &TouchPlan) -> anyhow::Result<String> {
    let table = quote_ident(&plan.table)?;
    let column = quote_ident(&plan.column)?;
    let function = quote_ident(&plan.function_name())?;
    let trigger = quote_ident(&plan.trigger_name())?;
    Ok(format!(
        "CREATE OR REPLACE FUNCTION {function}() RETURNS trigger
LANGUAGE plpgsql AS $tellurion_touch$
BEGIN
    -- {TOUCH_MARKER} {table}.{column}
    NEW.{column} := now();
    RETURN NEW;
END;
$tellurion_touch$;

CREATE OR REPLACE TRIGGER {trigger}
    BEFORE INSERT OR UPDATE ON {table}
    FOR EACH ROW EXECUTE FUNCTION {function}();"
    ))
}

/// Resolves the collection, its storage and its declared `modified_column`
/// out of a config — every refusal this command can make without touching a
/// database, in one place so all of them are assertable without one.
pub(crate) fn resolve_plan(config: &AppConfig, collection_id: &str) -> anyhow::Result<TouchPlan> {
    let (collection, storage) = resolve_collection(config, collection_id)?;

    if storage.driver != SUPPORTED_DRIVER {
        anyhow::bail!(
            "collection '{collection_id}' is served by storage '{}' (driver '{}'); a \
             modified-column touch trigger can only be provisioned for the '{SUPPORTED_DRIVER}' \
             driver, the one whose trigger dialect this command implements. A '{}' collection may \
             still declare a modified_column and serve the Optimistic Locking Timestamps class \
             off it — it just has to maintain that column some other way.",
            storage.id,
            storage.driver,
            storage.driver
        );
    }

    // The declaration is the source of truth. No `--column` flag, and no
    // derivation: a trigger maintaining a column no collection declares is a
    // write cost with no reader, and a trigger maintaining a column that
    // DISAGREES with the declaration is worse than none at all.
    let Some(column) = collection.modified_column.as_deref() else {
        anyhow::bail!(
            "collection '{collection_id}' declares no modified_column, so there is nothing for a \
             touch trigger to maintain. Declare one first (it must name a real timestamp column \
             the backend already reports — see the collection's 'modified_column' config key), \
             then rerun this command."
        );
    };

    Ok(TouchPlan {
        url_env: storage.url_env.clone(),
        table: tellurion_core::descriptor::target_table(collection).to_string(),
        column: column.to_string(),
    })
}

/// Resolves the collection and its storage out of a config — the same lookup
/// `variants::resolve_collection` performs, kept local for the same reason
/// every DDL module here keeps its own `quote_ident`.
fn resolve_collection<'a>(
    config: &'a AppConfig,
    collection_id: &str,
) -> anyhow::Result<(&'a CollectionDecl, &'a StorageDecl)> {
    let collection = config
        .collections
        .iter()
        .find(|c| c.id == collection_id)
        .ok_or_else(|| {
            anyhow::anyhow!("config declares no collection with id '{collection_id}'")
        })?;
    let storage = config
        .storages
        .iter()
        .find(|s| s.id == collection.storage)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "collection '{collection_id}' names storage '{}', which the config does not declare",
                collection.storage
            )
        })?;
    Ok((collection, storage))
}

pub async fn install(args: InstallArgs) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(&args.config)
        .with_context(|| format!("reading config '{}'", args.config.display()))?;
    let config: AppConfig = serde_yaml::from_str(&text)
        .with_context(|| format!("parsing config '{}'", args.config.display()))?;
    // The same shape validation the server runs at boot — `variants
    // materialize` does this too, and for the same reason: an operator
    // pointing this at a config the server would refuse should hear it here,
    // not after the DDL has landed.
    config
        .validate()
        .with_context(|| format!("validating config '{}'", args.config.display()))?;

    let plan = resolve_plan(&config, &args.collection)?;
    install_postgis(&plan, args.allow_existing_trigger, args.dry_run).await
}

/// One row of `information_schema.columns` for the declared column, in the
/// two facts this command cares about.
struct ColumnFacts {
    /// `data_type` — the broad spelling `PropertyType::from_sql_type`
    /// classifies, which is the same spelling `Router::validate_catalog`'s
    /// own `descriptor::reconcile_modified_column` check sees at boot. Read
    /// from the same view rather than re-derived, so the CLI's check and the
    /// server's cannot drift.
    data_type: String,
    /// `is_generated` — `'ALWAYS'` for a generated column, `'NEVER'`
    /// otherwise.
    is_generated: String,
}

/// Another trigger already on this table, as far as `pg_trigger` can describe
/// it.
struct ExistingTrigger {
    name: String,
    function: String,
    /// The function's own source text (`pg_proc.prosrc`) — used only to ask
    /// two questions: does it mention the declared column, and does it carry
    /// [`TOUCH_MARKER`]. Never parsed.
    source: String,
}

pub(crate) async fn install_postgis(
    plan: &TouchPlan,
    allow_existing_trigger: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    let sql = touch_trigger_sql(plan)?;
    // Always printed, dry run or not — the same "hand it to an operator
    // without CLI database access" requirement every DDL command in this
    // crate follows.
    println!("{sql}");
    if dry_run {
        return Ok(());
    }

    let client = crate::db::connect(&plan.url_env).await?;
    // `#272`: the preflight and the install go inside one advisory-locked
    // transaction, on the subject table's own name.
    //
    // Measured on this workspace's cluster, six concurrent sessions over
    // fifteen rounds: `CREATE OR REPLACE TRIGGER` alone failed 0 of 15 —
    // it takes a `ShareRowExclusiveLock` on the table, which conflicts with
    // itself, so two of them serialize. `CREATE OR REPLACE FUNCTION` alone
    // failed **9 of 15 rounds (13 of 90 sessions)** with `XX000 tuple
    // concurrently updated`: it locks nothing a second session waits on, so
    // two of them update the same `pg_proc` row at once. So this command
    // races on its first statement even though its second is safe, and the
    // error it produces is an internal-error SQLSTATE that says nothing
    // about what happened.
    //
    // The preflight is inside the lock for the same reason `seed`'s
    // ownership check is: it reads whether somebody else's trigger already
    // owns this column and then acts on the answer, and a second run
    // installing between the two would make that answer stale.
    crate::provision::refuse_overlong_identifiers(&sql)?;
    crate::provision::begin_locked(&client, &plan.table).await?;
    match preflight_and_install(&client, plan, allow_existing_trigger, &sql).await {
        Ok(()) => crate::provision::commit(&client).await?,
        Err(error) => {
            crate::provision::rollback(&client).await;
            return Err(error);
        }
    }
    tracing::info!(
        table = %plan.table,
        column = %plan.column,
        trigger = %plan.trigger_name(),
        "installed (or replaced) the modified-column touch trigger"
    );
    // Said out loud rather than left in a doc comment: this is the one
    // consequence an operator with application-side maintenance cannot
    // discover from the database, and the one this command cannot detect.
    println!(
        "-- '{}'.'{}' is now stamped with now() on every INSERT and UPDATE of the table, \
         including writes that never reach the server's write lane. If an application also \
         assigns this column, the trigger's value supersedes it within the same row version.",
        plan.table, plan.column
    );
    Ok(())
}

/// The two halves that must happen under one lock (`#272`) — unchanged from
/// before it existed, split out only so every way of leaving them, refusal
/// included, passes through the one `commit`/`rollback` above.
async fn preflight_and_install(
    client: &tokio_postgres::Client,
    plan: &TouchPlan,
    allow_existing_trigger: bool,
    sql: &str,
) -> anyhow::Result<()> {
    preflight(client, plan, allow_existing_trigger).await?;
    client.batch_execute(sql).await.with_context(|| {
        format!(
            "installing the modified-column touch trigger on '{}'.'{}'",
            plan.table, plan.column
        )
    })?;
    Ok(())
}

/// Every check that needs the live database, run before a single statement of
/// DDL. See the module doc for what each one is defending against and why
/// application-side maintenance is not among them.
async fn preflight(
    client: &tokio_postgres::Client,
    plan: &TouchPlan,
    allow_existing_trigger: bool,
) -> anyhow::Result<()> {
    let facts = column_facts(client, plan).await?;

    let classified = PropertyType::from_sql_type(&facts.data_type);
    if classified != PropertyType::DateTime {
        anyhow::bail!(
            "table '{}' column '{}' has SQL type '{}', which classifies as '{}' rather than a \
             timestamp. A touch trigger assigning now() to it would either fail or store a \
             coerced value the server's own Last-Modified parsing could not read — the same rule \
             the server applies at boot (descriptor::reconcile_modified_column).",
            plan.table,
            plan.column,
            facts.data_type,
            classified.as_str()
        );
    }

    if facts.is_generated != "NEVER" {
        anyhow::bail!(
            "table '{}' column '{}' is GENERATED ALWAYS ('is_generated' = '{}'); a trigger cannot \
             assign to a generated column, so this trigger would fail on the first write rather \
             than maintain anything. Drop the generation expression, or maintain the column with \
             it.",
            plan.table,
            plan.column,
            facts.is_generated
        );
    }

    let function_owner_conflict = foreign_function_named(client, plan).await?;
    if function_owner_conflict {
        anyhow::bail!(
            "a function named '{}' already exists and this command did not create it (its body \
             carries no '{TOUCH_MARKER}' marker). Refusing to replace it — rename it, or drop it \
             if it is dead.",
            plan.function_name()
        );
    }

    let competing = competing_triggers(client, plan).await?;
    if !competing.is_empty() && !allow_existing_trigger {
        let listed = competing
            .iter()
            .map(|t| format!("'{}' (function '{}')", t.name, t.function))
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "table '{}' already carries {} row-level trigger(s) whose function body mentions \
             column '{}': {listed}. Two triggers assigning one column run in trigger-name order, \
             which is not an ordering this command will silently establish. Drop the other \
             trigger if it is what maintained this column until now, or pass \
             --allow-existing-trigger if its body only reads the column.",
            plan.table,
            competing.len(),
            plan.column
        );
    }
    Ok(())
}

async fn column_facts(
    client: &tokio_postgres::Client,
    plan: &TouchPlan,
) -> anyhow::Result<ColumnFacts> {
    // Schema-qualified to `public`, the same schema every other PostGIS path
    // in this crate reads (`variants`'s `f_table_schema = 'public'`).
    let rows = client
        .query(
            "SELECT column_name, data_type, is_generated FROM information_schema.columns \
             WHERE table_schema = 'public' AND table_name = $1",
            &[&plan.table],
        )
        .await
        .with_context(|| {
            format!(
                "reading information_schema.columns for table '{}'",
                plan.table
            )
        })?;
    if rows.is_empty() {
        anyhow::bail!(
            "table 'public.{}' does not exist (or reports no columns); provision it first — this \
             command adds a trigger to a table, it never creates one.",
            plan.table
        );
    }
    let found = rows
        .iter()
        .find(|row| row.get::<_, String>(0) == plan.column);
    let Some(row) = found else {
        anyhow::bail!(
            "table '{}' reports no column named '{}', which is the modified_column this \
             collection declares. The server refuses the same declaration at boot \
             (descriptor::reconcile_modified_column); provision the column first.",
            plan.table,
            plan.column
        );
    };
    Ok(ColumnFacts {
        data_type: row.get(1),
        is_generated: row.get(2),
    })
}

/// Whether a function already occupies this plan's derived name without
/// carrying [`TOUCH_MARKER`] — i.e. something this command did not create and
/// must not `CREATE OR REPLACE` over.
async fn foreign_function_named(
    client: &tokio_postgres::Client,
    plan: &TouchPlan,
) -> anyhow::Result<bool> {
    let rows = client
        .query(
            "SELECT p.prosrc FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
             WHERE n.nspname = 'public' AND p.proname = $1",
            &[&plan.function_name()],
        )
        .await
        .with_context(|| format!("reading pg_proc for function '{}'", plan.function_name()))?;
    Ok(rows
        .iter()
        .any(|row| !row.get::<_, String>(0).contains(TOUCH_MARKER)))
}

/// Row-level triggers already on this table, other than this plan's own,
/// whose function body mentions the declared column. See the module doc for
/// why "mentions" is the honest test and why it is not conclusive in either
/// direction.
async fn competing_triggers(
    client: &tokio_postgres::Client,
    plan: &TouchPlan,
) -> anyhow::Result<Vec<ExistingTrigger>> {
    let rows = client
        .query(
            "SELECT t.tgname, p.proname, p.prosrc \
             FROM pg_trigger t \
             JOIN pg_proc p ON p.oid = t.tgfoid \
             JOIN pg_class c ON c.oid = t.tgrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'public' AND c.relname = $1 AND NOT t.tgisinternal",
            &[&plan.table],
        )
        .await
        .with_context(|| format!("reading pg_trigger for table '{}'", plan.table))?;
    let own = plan.trigger_name();
    Ok(rows
        .iter()
        .map(|row| ExistingTrigger {
            name: row.get(0),
            function: row.get(1),
            source: row.get(2),
        })
        .filter(|t| t.name != own)
        // A touch trigger this command installed for a DIFFERENT column is
        // not a competitor: it assigns its own column and says so with the
        // marker.
        .filter(|t| !t.source.contains(TOUCH_MARKER))
        .filter(|t| t.source.contains(&plan.column))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(table: &str, column: &str) -> TouchPlan {
        TouchPlan {
            url_env: "DATABASE_URL".to_string(),
            table: table.to_string(),
            column: column.to_string(),
        }
    }

    fn config_yaml(driver: &str, modified_column: Option<&str>) -> AppConfig {
        let mut yaml = format!(
            "control_store:\n  backend: legacy_file\nserver:\n  port: 8080\nstorages:\n  \
             - id: main\n    driver: {driver}\n    url_env: DATABASE_URL\ntenants:\n  \
             - id: public\ncatalogs:\n  - id: default\n    tenant: public\ncollections:\n  \
             - id: demo\n    catalog: default\n    storage: main\n"
        );
        if let Some(column) = modified_column {
            yaml.push_str(&format!("    modified_column: {column}\n"));
        }
        serde_yaml::from_str(&yaml).expect("test config parses")
    }

    // -- SQL text -----------------------------------------------------------

    /// The three properties the trigger's correctness rests on, asserted
    /// together because each one alone reads as an arbitrary choice: it fires
    /// on INSERT as well as UPDATE (so a `PUT`-created row is stamped), it
    /// carries no `WHEN` clause (so the column moves exactly when `xmin`
    /// does — see the module doc's `#150` section), and it assigns `now()`
    /// rather than `clock_timestamp()`.
    #[test]
    fn the_trigger_fires_on_insert_and_update_unconditionally_with_now() {
        let sql = touch_trigger_sql(&plan("demo", "modified")).unwrap();
        assert!(
            sql.contains("BEFORE INSERT OR UPDATE ON \"demo\""),
            "the insert arm is what stamps a PUT-created row: {sql}"
        );
        assert!(
            !sql.contains("WHEN ("),
            "a WHEN guard would let an update bump xmin without moving the column: {sql}"
        );
        assert!(sql.contains("NEW.\"modified\" := now();"), "{sql}");
        assert!(!sql.contains("clock_timestamp"), "{sql}");
        assert!(sql.contains("FOR EACH ROW"), "{sql}");
    }

    /// A rerun must be a no-op, and must never leave a window in which the
    /// table has no trigger — so both halves are `CREATE OR REPLACE` and
    /// neither is a `DROP`.
    #[test]
    fn the_ddl_is_idempotent_without_ever_dropping_anything() {
        let sql = touch_trigger_sql(&plan("demo", "modified")).unwrap();
        assert!(
            sql.contains("CREATE OR REPLACE FUNCTION \"demo_modified_touch\"()"),
            "{sql}"
        );
        assert!(
            sql.contains("CREATE OR REPLACE TRIGGER \"demo_modified_touch_trg\""),
            "{sql}"
        );
        assert!(!sql.contains("DROP"), "{sql}");
    }

    /// Ownership is a fact about the database, not a naming convention — the
    /// marker has to reach `pg_proc.prosrc`, which means inside the dollar-
    /// quoted body, not above the `CREATE`.
    #[test]
    fn the_marker_lands_inside_the_function_body() {
        let sql = touch_trigger_sql(&plan("demo", "modified")).unwrap();
        let body_start = sql.find("$tellurion_touch$").expect("body opens");
        let body_end = sql.rfind("$tellurion_touch$").expect("body closes");
        assert!(body_start < body_end);
        assert!(sql[body_start..body_end].contains(TOUCH_MARKER), "{sql}");
    }

    #[test]
    fn names_are_derived_from_both_the_table_and_the_column() {
        // A table may carry one touch trigger per maintained column, so the
        // column has to be in both derived names.
        let a = plan("demo", "modified");
        let b = plan("demo", "updated");
        assert_ne!(a.function_name(), b.function_name());
        assert_ne!(a.trigger_name(), b.trigger_name());
    }

    #[test]
    fn rejects_a_table_or_column_that_fails_identifier_whitelisting() {
        assert!(touch_trigger_sql(&plan("demo; DROP TABLE x; --", "modified")).is_err());
        assert!(touch_trigger_sql(&plan("demo", "modified\" := 'x'; --")).is_err());
    }

    /// The derived names are longer than the table name, so a table name that
    /// is itself legal can still produce an over-length trigger name. Refused
    /// rather than silently truncated by Postgres — a truncated name is a
    /// trigger no rerun of this command could find again.
    #[test]
    fn refuses_a_derived_name_postgres_would_silently_truncate() {
        let long_table = "t".repeat(60);
        let plan = plan(&long_table, "modified");
        assert!(plan.function_name().len() > 63);
        assert!(touch_trigger_sql(&plan).is_err());
    }

    // -- config resolution --------------------------------------------------

    #[test]
    fn resolves_the_table_and_column_from_the_declaration_alone() {
        let config = config_yaml("postgis", Some("modified"));
        let resolved = resolve_plan(&config, "demo").unwrap();
        assert_eq!(resolved.table, "demo");
        assert_eq!(resolved.column, "modified");
        assert_eq!(resolved.url_env, "DATABASE_URL");
    }

    /// The refusal `#151` asks for by name: any driver but PostGIS, named,
    /// rather than a dialect this command has not proved.
    #[test]
    fn refuses_a_geopackage_collection_naming_the_driver() {
        let config = config_yaml("geopackage", Some("modified"));
        let err = resolve_plan(&config, "demo").unwrap_err().to_string();
        assert!(err.contains("geopackage"), "{err}");
        assert!(err.contains("postgis"), "{err}");
    }

    /// No declaration, no trigger — the column the trigger maintains is never
    /// this command's own invention.
    #[test]
    fn refuses_a_collection_that_declares_no_modified_column() {
        let config = config_yaml("postgis", None);
        let err = resolve_plan(&config, "demo").unwrap_err().to_string();
        assert!(err.contains("declares no modified_column"), "{err}");
    }

    #[test]
    fn refuses_a_collection_the_config_does_not_declare() {
        let config = config_yaml("postgis", Some("modified"));
        let err = resolve_plan(&config, "nope").unwrap_err().to_string();
        assert!(err.contains("nope"), "{err}");
    }

    // -- live database ------------------------------------------------------

    fn test_database_url() -> Option<String> {
        std::env::var("TELLURION_TEST_DATABASE_URL").ok()
    }

    /// `install_postgis` reads its connection string out of the environment
    /// variable the plan names, so a live test points that name straight at
    /// `TELLURION_TEST_DATABASE_URL` rather than needing a second variable.
    fn live_plan(table: &str, column: &str) -> TouchPlan {
        TouchPlan {
            url_env: "TELLURION_TEST_DATABASE_URL".to_string(),
            table: table.to_string(),
            column: column.to_string(),
        }
    }

    async fn live_client(url: &str) -> tokio_postgres::Client {
        crate::db::connect_url(url)
            .await
            .expect("connect to the test database")
    }

    async fn drop_table(client: &tokio_postgres::Client, table: &str) {
        client
            .batch_execute(&format!("DROP TABLE IF EXISTS \"{table}\" CASCADE"))
            .await
            .expect("drop any leftover table from a previous run");
    }

    /// A table shaped like a collection that declares `modified` — one
    /// timestamp column, one ordinary attribute, no default and no
    /// application-side maintenance of any kind.
    ///
    /// `#272`: through `#138`'s harness, under the subject table's own name
    /// — which is the same name `install_postgis` now locks, so this fixture
    /// and the install it sets up exclude each other rather than racing.
    async fn create_subject_table(client: &tokio_postgres::Client, table: &str) {
        tellurion_postgis::test_harness::apply_fixture_ddl(
            client,
            table,
            &format!(
                "CREATE TABLE \"{table}\" (
                     id integer PRIMARY KEY,
                     name text,
                     modified timestamptz
                 )"
            ),
        )
        .await
        .expect("create the subject table");
    }

    async fn modified_of(client: &tokio_postgres::Client, table: &str, id: i32) -> String {
        client
            .query_one(
                &format!(
                    "SELECT COALESCE(modified::text, '<null>') FROM \"{table}\" WHERE id = $1"
                ),
                &[&id],
            )
            .await
            .expect("read the modified column")
            .get(0)
    }

    /// **The decisive test** (`#151`).
    ///
    /// Everything here is written through raw SQL against the table — the
    /// DBA / bulk-fix-up / `ogr2ogr` path, categorically NOT the server's
    /// write lane and categorically invisible to any Rust-side touch. The
    /// negative half runs first, on the same table, so "the column moved"
    /// cannot be an artifact of the column moving on its own:
    ///
    /// 1. with no trigger provisioned, an `UPDATE` leaves `modified`
    ///    untouched — today's behaviour, which a deployment that never runs
    ///    this command keeps;
    /// 2. after `install_postgis`, the same `UPDATE` moves it;
    /// 3. and an `INSERT` that explicitly supplies an old value has it
    ///    superseded, which is the arm `#151`'s own `BEFORE UPDATE` sketch
    ///    would have left open for a `PUT`-created row.
    #[tokio::test]
    async fn the_trigger_stamps_writes_that_never_reach_the_write_lane() {
        let Some(url) = test_database_url() else {
            eprintln!(
                "skipping the_trigger_stamps_writes_that_never_reach_the_write_lane: TELLURION_TEST_DATABASE_URL not set"
            );
            return;
        };
        let table = "tellurion_touch_trigger_decisive";
        let client = live_client(&url).await;
        drop_table(&client, table).await;
        create_subject_table(&client, table).await;

        let stale = "2001-01-01 00:00:00+00";
        client
            .execute(
                &format!(
                    "INSERT INTO \"{table}\" (id, name, modified) VALUES (1, 'before', $1::text::timestamptz)"
                ),
                &[&stale],
            )
            .await
            .expect("seed a row the way a bulk load would");

        // 1. The negative. No trigger yet, so a write outside the write lane
        //    leaves the declared column exactly where it was — which is
        //    precisely why `If-Unmodified-Since` is a no-op guard today.
        client
            .execute(
                &format!("UPDATE \"{table}\" SET name = 'untouched-run' WHERE id = 1"),
                &[],
            )
            .await
            .expect("update outside the write lane");
        let before_install = modified_of(&client, table, 1).await;
        assert!(
            before_install.starts_with("2001-01-01"),
            "without the trigger provisioned an out-of-lane UPDATE must leave the column alone, \
             got {before_install}"
        );

        // 2. Provision it. Twice — a rerun must be a no-op, not an error.
        let plan = live_plan(table, "modified");
        install_postgis(&plan, false, false)
            .await
            .expect("install the touch trigger");
        install_postgis(&plan, false, false)
            .await
            .expect("reinstalling the touch trigger is idempotent");

        // 3. The positive. The identical write, through the identical path.
        client
            .execute(
                &format!("UPDATE \"{table}\" SET name = 'touched-run' WHERE id = 1"),
                &[],
            )
            .await
            .expect("update outside the write lane, with the trigger installed");
        let after_install = modified_of(&client, table, 1).await;
        assert!(
            !after_install.starts_with("2001-01-01"),
            "with the trigger installed the same out-of-lane UPDATE must move the column, got \
             {after_install}"
        );
        let moved: bool = client
            .query_one(
                &format!(
                    "SELECT modified > now() - interval '1 hour' FROM \"{table}\" WHERE id = 1"
                ),
                &[],
            )
            .await
            .expect("compare the stamp against now()")
            .get(0);
        assert!(moved, "the stamp must be now(), got {after_install}");

        // 4. The insert arm: an explicitly supplied stale value is
        //    superseded, so a row created by a `PUT` against a new id is
        //    stamped too.
        client
            .execute(
                &format!(
                    "INSERT INTO \"{table}\" (id, name, modified) VALUES (2, 'created', $1::text::timestamptz)"
                ),
                &[&stale],
            )
            .await
            .expect("insert with the trigger installed");
        let inserted = modified_of(&client, table, 2).await;
        assert!(
            !inserted.starts_with("2001-01-01"),
            "the INSERT arm must stamp a newly created row, got {inserted}"
        );

        drop_table(&client, table).await;
    }

    /// The trigger writes `NEW` inside the statement that is already writing
    /// the row, so it produces ONE row version, not two — the property
    /// `#150`'s `xmin` witness depends on. A second version would mean an
    /// `apply_conditional` whose `WHERE ... AND xmin = $witness` matched and
    /// then immediately went stale.
    #[tokio::test]
    async fn the_trigger_produces_one_row_version_per_write() {
        let Some(url) = test_database_url() else {
            eprintln!(
                "skipping the_trigger_produces_one_row_version_per_write: TELLURION_TEST_DATABASE_URL not set"
            );
            return;
        };
        let table = "tellurion_touch_trigger_xmin";
        let mut client = live_client(&url).await;
        drop_table(&client, table).await;
        create_subject_table(&client, table).await;
        client
            .batch_execute(&format!(
                "INSERT INTO \"{table}\" (id, name) VALUES (1, 'a')"
            ))
            .await
            .expect("seed");

        install_postgis(&live_plan(table, "modified"), false, false)
            .await
            .expect("install the touch trigger");

        // One transaction, one UPDATE. `xmin` must be that transaction's own
        // id — the same value `txid_current()` reports — which it can only be
        // if exactly one tuple version was written.
        let tx = client
            .transaction()
            .await
            .expect("open a transaction of our own");
        tx.execute(
            &format!("UPDATE \"{table}\" SET name = 'b' WHERE id = 1"),
            &[],
        )
        .await
        .expect("update inside our transaction");
        let same: bool = tx
            .query_one(
                &format!(
                    "SELECT xmin::text::bigint = txid_current() FROM \"{table}\" WHERE id = 1"
                ),
                &[],
            )
            .await
            .expect("compare xmin against our own transaction id")
            .get(0);
        assert!(
            same,
            "a BEFORE trigger must rewrite NEW in place, leaving one row version whose xmin is \
             this transaction's own"
        );
        // And the column moved in that same single version.
        let stamped: bool = tx
            .query_one(
                &format!("SELECT modified IS NOT NULL FROM \"{table}\" WHERE id = 1"),
                &[],
            )
            .await
            .expect("read the stamp")
            .get(0);
        assert!(stamped);
        tx.commit().await.expect("commit");

        drop_table(&client, table).await;
    }

    /// A column a trigger cannot assign is refused before any DDL lands,
    /// naming the column — not left to fail on the first write.
    #[tokio::test]
    async fn refuses_a_generated_column_by_name() {
        let Some(url) = test_database_url() else {
            eprintln!(
                "skipping refuses_a_generated_column_by_name: TELLURION_TEST_DATABASE_URL not set"
            );
            return;
        };
        let table = "tellurion_touch_trigger_generated";
        let client = live_client(&url).await;
        drop_table(&client, table).await;
        client
            .batch_execute(&format!(
                "CREATE TABLE \"{table}\" (
                     id integer PRIMARY KEY,
                     created timestamp NOT NULL,
                     modified timestamp GENERATED ALWAYS AS (created + interval '1 day') STORED
                 )"
            ))
            .await
            .expect("create a table whose modified column is generated");

        let err = install_postgis(&live_plan(table, "modified"), false, false)
            .await
            .expect_err("a generated column must be refused")
            .to_string();
        assert!(err.contains("GENERATED ALWAYS"), "{err}");
        assert!(err.contains("modified"), "{err}");

        drop_table(&client, table).await;
    }

    /// A column of the wrong type is refused with the same rule the server
    /// applies at boot, rather than installing a trigger that would store a
    /// value `Last-Modified` could never parse.
    #[tokio::test]
    async fn refuses_a_column_that_is_not_a_timestamp() {
        let Some(url) = test_database_url() else {
            eprintln!(
                "skipping refuses_a_column_that_is_not_a_timestamp: TELLURION_TEST_DATABASE_URL not set"
            );
            return;
        };
        let table = "tellurion_touch_trigger_wrongtype";
        let client = live_client(&url).await;
        drop_table(&client, table).await;
        client
            .batch_execute(&format!(
                "CREATE TABLE \"{table}\" (id integer PRIMARY KEY, modified text)"
            ))
            .await
            .expect("create a table whose modified column is text");

        let err = install_postgis(&live_plan(table, "modified"), false, false)
            .await
            .expect_err("a text column must be refused")
            .to_string();
        assert!(err.contains("classifies as"), "{err}");

        drop_table(&client, table).await;

        // And a missing column is its own named refusal, not a confusing
        // failure from the DDL itself.
        drop_table(&client, table).await;
        client
            .batch_execute(&format!(
                "CREATE TABLE \"{table}\" (id integer PRIMARY KEY)"
            ))
            .await
            .expect("create a table with no modified column at all");
        let err = install_postgis(&live_plan(table, "modified"), false, false)
            .await
            .expect_err("a missing column must be refused")
            .to_string();
        assert!(err.contains("reports no column named 'modified'"), "{err}");
        drop_table(&client, table).await;
    }

    /// The "already maintained" case this command CAN see: another row-level
    /// trigger whose body mentions the column. Refused by name, and
    /// `--allow-existing-trigger` is the explicit way through.
    #[tokio::test]
    async fn refuses_a_competing_trigger_by_name_unless_consented_to() {
        let Some(url) = test_database_url() else {
            eprintln!(
                "skipping refuses_a_competing_trigger_by_name_unless_consented_to: TELLURION_TEST_DATABASE_URL not set"
            );
            return;
        };
        let table = "tellurion_touch_trigger_competing";
        let client = live_client(&url).await;
        drop_table(&client, table).await;
        create_subject_table(&client, table).await;
        // Exactly what an operator who already maintains the column in the
        // database looks like from `pg_trigger`.
        client
            .batch_execute(&format!(
                "CREATE OR REPLACE FUNCTION {table}_existing_touch() RETURNS trigger
                 LANGUAGE plpgsql AS $existing$
                 BEGIN
                     NEW.modified := now();
                     RETURN NEW;
                 END;
                 $existing$;
                 CREATE OR REPLACE TRIGGER {table}_existing_trg
                     BEFORE UPDATE ON \"{table}\"
                     FOR EACH ROW EXECUTE FUNCTION {table}_existing_touch();"
            ))
            .await
            .expect("install the operator's own pre-existing maintenance");

        let plan = live_plan(table, "modified");
        let err = install_postgis(&plan, false, false)
            .await
            .expect_err("a competing trigger must be refused")
            .to_string();
        assert!(err.contains(&format!("{table}_existing_trg")), "{err}");
        assert!(err.contains("--allow-existing-trigger"), "{err}");

        // Nothing landed: the refusal is before any DDL, so the table still
        // carries exactly the one trigger it started with.
        let count: i64 = client
            .query_one(
                "SELECT count(*) FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid \
                 WHERE c.relname = $1 AND NOT t.tgisinternal",
                &[&table],
            )
            .await
            .expect("count triggers")
            .get(0);
        assert_eq!(count, 1, "a refusal must not have installed anything");

        // Consent is the way through.
        install_postgis(&plan, true, false)
            .await
            .expect("--allow-existing-trigger installs alongside");

        client
            .batch_execute(&format!(
                "DROP FUNCTION IF EXISTS {table}_existing_touch() CASCADE"
            ))
            .await
            .expect("clean up");
        drop_table(&client, table).await;
    }

    /// A function already occupying the derived name that this command did
    /// not create is never silently replaced — ownership is read from the
    /// marker in `pg_proc.prosrc`, not assumed from the name.
    #[tokio::test]
    async fn refuses_to_replace_a_function_it_did_not_create() {
        let Some(url) = test_database_url() else {
            eprintln!(
                "skipping refuses_to_replace_a_function_it_did_not_create: TELLURION_TEST_DATABASE_URL not set"
            );
            return;
        };
        let table = "tellurion_touch_trigger_owned";
        let client = live_client(&url).await;
        drop_table(&client, table).await;
        create_subject_table(&client, table).await;
        let plan = live_plan(table, "modified");
        let squatter = plan.function_name();
        client
            .batch_execute(&format!(
                "CREATE OR REPLACE FUNCTION \"{squatter}\"() RETURNS trigger
                 LANGUAGE plpgsql AS $squat$ BEGIN RETURN NEW; END; $squat$;"
            ))
            .await
            .expect("squat on the derived function name");

        let err = install_postgis(&plan, false, false)
            .await
            .expect_err("a foreign function of the same name must be refused")
            .to_string();
        assert!(err.contains(&squatter), "{err}");
        assert!(err.contains(TOUCH_MARKER), "{err}");

        client
            .batch_execute(&format!("DROP FUNCTION IF EXISTS \"{squatter}\"() CASCADE"))
            .await
            .expect("clean up");
        drop_table(&client, table).await;
    }
}
