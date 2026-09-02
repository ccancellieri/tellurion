//! Authorization policy layer (`#34`, built on top of `#17`/`#34`'s
//! OIDC/membership authentication in `auth.rs`) — RBAC + ABAC evaluated
//! post-resolution, once a request's tenant/catalog/collection external ids
//! have already been resolved to internal ones (`resolver.rs`). This module
//! never runs before resolution and never sees an external id; a protocol
//! handler calls [`authorize_resource`] right after its own
//! `Router::resolve_features`/`resolve_tiles`/... call, with exactly the
//! internal ids that call already produced. See
//! `docs/design/2026-07-18-authorization-policy-layer.md` for the full
//! design writeup this module implements.
//!
//! ## Two independent gates
//!
//! [`authorize_resource`] evaluates two independent conditions, in order:
//!
//! 1. **Platform isolation** (authorization directive 2/3, non-overridable):
//!    a subject may see a resource only if it holds membership in the
//!    resource's own owning tenant, or the resource's
//!    [`VisibilityDecl`](crate::config::VisibilityDecl) marks it `public`,
//!    or lists a tenant the subject holds membership in under
//!    `shared_with`. Fails this and the decision is
//!    [`PolicyDecision::Deny`] outright — no role or grant can override it.
//! 2. **RBAC/ABAC** (directives 4/5): once isolation passes, does this
//!    tenant have *any* policy active at all (`policy.rs`'s own per-tenant
//!    activation rule, below)? If not, access is unrestricted — today's
//!    behavior, unchanged (directive 10). If so, the subject must hold at
//!    least one role, in the resource's own tenant, with a grant covering
//!    this collection/lane; the matching grants' filters are combined (see
//!    below) into the decision's effective filter.
//!
//! ## Per-tenant RBAC activation (directive 10's generalization)
//!
//! A tenant's role table is the platform-shared roles
//! (`PolicyConfig::roles`) overlaid by that tenant's own tenant-custom
//! document (`PolicyConfig::tenant_policies`), if one exists — a role name
//! declared in both has the tenant-custom grants win outright (whole-role
//! replacement, the same "nearest wins, maps replace whole" rule
//! `SettingsDecl` already uses). If that overlaid table is **empty** for a
//! tenant (no platform roles declared at all, and no tenant-custom document
//! for that tenant), RBAC is inactive for that tenant: a subject who cleared
//! isolation reads unrestricted, exactly as every deployment behaved before
//! this module existed. Once a tenant's table is non-empty, RBAC is
//! default-deny for that tenant: a subject with no matching grant is denied,
//! even if they hold plain tenant membership.
//!
//! Cross-tenant reads (a subject who cleared isolation via `public`/
//! `shared_with` rather than membership in the resource's own tenant) never
//! consult RBAC at all — directive 1 ties every role strictly to a
//! `(tenant, role)` membership pair, and a non-member by definition holds no
//! role in the resource's tenant, so no grant could ever match for them.
//! Such a read is always unrestricted once isolation passes. An operator
//! wanting *filtered* cross-tenant access needs a role granted to the
//! reading subject's own tenant, evaluated when that subject reads via a
//! grant scoped under their own tenant's policy — modeling "tenant B's
//! members read tenant A's data, filtered" is not yet supported in this
//! slice; see the design doc's "Out of scope" section.
//!
//! ## Claim substitution and missing-claim behavior (directive 5)
//!
//! A grant's `filter` is CQL2-text with `{{claims.NAME}}` placeholders,
//! substituted from `Subject::claims` before parsing. **A placeholder whose
//! claim is absent from the subject makes that grant unsatisfied for this
//! subject** — the grant is excluded from evaluation entirely, not treated
//! as an error and not silently dropped down to "unfiltered." This is a
//! deliberate, conservative default: a grant that says "only rows where
//! `org = {{claims.org}}`" for a subject with no `org` claim has no honest
//! answer to substitute, and guessing wrong in either direction (unfiltered,
//! or a filter that matches nothing) is worse than simply not counting that
//! grant as a match. If no OTHER matching grant is satisfied either, the
//! decision is `Deny`.
//!
//! ## Which lanes can push a filter down
//!
//! Multiple matching, satisfied grants combine as OR (any one of them being
//! unrestricted makes the whole decision unrestricted); the AND-merge with a
//! request's own user-supplied filter happens at the caller (each protocol
//! handler), not here — see `authorize_resource`'s own doc. Whether a lane
//! *can* push its resulting filter into the actual fetch is a property of
//! the resolved driver, not a fixed property of the lane: every caller
//! passes `lane_supports_filter` as the resolved source's own
//! `filter_capable()` (`FeatureSource::filter_capable`/`TileSource::
//! filter_capable`) — `tellurion-features`' items-list and single-item GET,
//! `tellurion-stac`'s item-search/items-list/single-item GET, and
//! `tellurion-tiles`'/`tellurion-places`' vector/styled/rendered-PNG/glb
//! tile lanes (all sharing one MVT-first fetch, `fetch_mvt`) all reach here
//! this way. PostGIS advertises `filter_capable() == true` on both traits
//! (`sql::compile_filter` compiles the AND-merged filter into the driver's
//! own `WHERE` clause, for a single-row lookup and an MVT tile query alike,
//! exactly as it already did for the items-list query); PMTiles and every
//! attribute-filter-incapable driver (FlatGeobuf, GeoParquet) stay at the
//! trait default (`false`) — a pre-baked archive has no query to narrow, and
//! FlatGeobuf/GeoParquet never claimed CQL2 compilation to begin with (`#33`).
//! A lane whose resolved driver reports `false` still DENIES outright rather
//! than silently serving unfiltered whenever the only matching grants
//! require a filter — see this function's own doc. Directive 5 anticipates
//! exactly this shape for pre-baked tile archives ("grants there are
//! collection-level allow/deny only"); this module extends the same
//! treatment, driver-by-driver, to every lane whose resolved source can't
//! compile a filter, not only PMTiles.
//!
//! The tile lanes additionally partition their shared cache by the
//! requesting subject's effective filter — see
//! [`crate::cache::TileKey::policy_fingerprint`]'s own doc for the exact
//! fingerprint composition and cache-sharing rules this feeds.
//!
//! ## A third gate, charged separately: rate ceilings (`#188`)
//!
//! A grant may also carry a fixed-window request-rate ceiling
//! ([`crate::rate_limit::RateLimitDecl`]). That condition is deliberately
//! NOT evaluated by [`authorize_resource`]: authorization asks "may this
//! subject see this resource," a question with the same answer however many
//! times it is asked, while a ceiling *consumes budget* and so may be
//! charged exactly once per served request. Folding the two together would
//! have made a collections listing — which runs the authorization
//! checkpoint once per candidate collection purely to decide what to list —
//! spend one request's budget N times.
//!
//! So [`enforce_rate_limits`] is a separate call the caller makes after an
//! `Allow`, naming with [`RateCharge`] whether this checkpoint stands for a
//! served request or is only a visibility probe. See that function's own
//! doc for how several matching grants' ceilings compose (conjunctively —
//! unlike filters, which are permissions and compose as OR).

use std::collections::HashMap;

use crate::auth::Subject;
use crate::config::{AppConfig, GrantDecl, PolicyLane, RoleDecl, VisibilityDecl};
use crate::error::{Error, Result};
use crate::filter::{self, Filter};
use crate::rate_limit::{
    evaluate_condition, ConditionOutcome, RateCharge, RateCounter, RateRefusal, RateScope,
    RateVerdict,
};

/// One resource, already resolved to internal ids, that a protocol handler
/// is about to serve — the sole input [`authorize_resource`] needs beyond
/// `AppConfig`/[`Subject`] itself. Deliberately carries no external id: this
/// module runs strictly post-resolution (see the module doc).
pub struct ResourceContext<'a> {
    pub tenant_id: &'a str,
    pub catalog_id: &'a str,
    pub collection_id: &'a str,
    pub lane: PolicyLane,
    /// This resource's effective visibility — `Router::effective_visibility`
    /// already resolved the catalog/collection two-level inheritance; this
    /// module treats it as an opaque input.
    pub visibility: &'a VisibilityDecl,
}

/// [`authorize_resource`]'s verdict.
#[derive(Debug, Clone, PartialEq)]
pub enum PolicyDecision {
    /// Access is allowed. `filter` is `Some` when the subject's access is
    /// narrowed by ABAC (directive 5) — `None` means unrestricted, whether
    /// because RBAC is inactive for this tenant, the subject holds an
    /// unconditional grant, or isolation passed via `public`/`shared_with`
    /// (cross-tenant reads are always unrestricted — see the module doc).
    Allow {
        filter: Option<Filter>,
    },
    Deny,
}

/// Evaluates whether `subject` may read `resource` (the two-gate design the
/// module doc describes: isolation, then RBAC/ABAC), and — for a lane that
/// can push a filter down (`lane_supports_filter: true`) — what filter (if
/// any) to AND-merge into the query. `config` supplies both `policy.roles`/
/// `policy.tenant_policies` (RBAC) and nothing else; `resource.visibility`
/// already carries the resolved isolation input, so this function does no
/// `Router` lookups of its own.
///
/// Returns `Err` only for a policy *misconfiguration* discovered at
/// evaluation time — a grant's filter template, once claims are substituted,
/// no longer parses as CQL2-text. This is deliberately distinct from
/// `PolicyDecision::Deny`: a malformed policy is a server-side authoring
/// bug (should surface as 500, the caller's job to map), never something a
/// well-behaved client caused (which is always `Deny`, never `Err`).
/// `AppConfig::validate`'s own eager, dummy-substituted syntax check catches
/// most such bugs at boot already — this is the defense-in-depth path for
/// what that check cannot see (a real claim value producing a shape the
/// dummy placeholder didn't).
pub fn authorize_resource(
    config: &AppConfig,
    resource: &ResourceContext,
    subject: &Subject,
    lane_supports_filter: bool,
) -> Result<PolicyDecision> {
    // --- Gate 1: platform isolation (directive 2/3, non-overridable) -----
    let is_member = subject.is_member_of(resource.tenant_id);
    let is_shared = resource.visibility.public
        || resource
            .visibility
            .shared_with
            .iter()
            .any(|tenant| subject.is_member_of(tenant));
    if !is_member && !is_shared {
        return Ok(PolicyDecision::Deny);
    }

    // Cross-tenant reads (isolation passed via public/shared, not
    // membership) never consult RBAC — directive 1 ties every role to a
    // `(tenant, role)` pair the subject does not hold in this tenant. See
    // the module doc's "Per-tenant RBAC activation" section.
    if !is_member {
        return Ok(PolicyDecision::Allow { filter: None });
    }

    // --- Gate 2: RBAC/ABAC (directives 4/5) -------------------------------
    let role_table = resolve_role_table(config, resource.tenant_id);
    if role_table.is_empty() {
        // RBAC inactive for this tenant — unrestricted, unchanged behavior.
        return Ok(PolicyDecision::Allow { filter: None });
    }

    let held_roles = subject
        .memberships
        .get(resource.tenant_id)
        .cloned()
        .unwrap_or_default();

    let mut matching_filters: Vec<Filter> = Vec::new();
    let mut any_unconditional = false;
    for role_name in &held_roles {
        let Some(grants) = role_table.get(role_name.as_str()) else {
            continue;
        };
        for grant in *grants {
            if !grant.lanes.contains(&resource.lane) {
                continue;
            }
            if !grant
                .scope
                .matches(resource.catalog_id, resource.collection_id)
            {
                continue;
            }
            match &grant.filter {
                None => {
                    any_unconditional = true;
                }
                // Missing claim: `substitute_claims` returns `None`, and
                // this grant is simply not satisfied for this subject —
                // excluded, not an error, not "unfiltered". See the module
                // doc's "Claim substitution" section.
                Some(template) => {
                    if let Some(substituted) = substitute_claims(template, &subject.claims) {
                        let parsed = filter::parse_text(&substituted).map_err(|source| {
                            Error::Config(format!(
                                "policy grant for role '{role_name}' has a filter that no longer parses as CQL2-text once claims are substituted: {source}"
                            ))
                        })?;
                        matching_filters.push(parsed);
                    }
                }
            }
        }
    }

    if any_unconditional {
        return Ok(PolicyDecision::Allow { filter: None });
    }
    if matching_filters.is_empty() {
        // No role held here matched any grant for this lane/scope (or every
        // matching grant's claim substitution failed) — default deny.
        return Ok(PolicyDecision::Deny);
    }

    let effective_filter = if matching_filters.len() == 1 {
        matching_filters
            .into_iter()
            .next()
            .expect("checked len == 1")
    } else {
        Filter::Or(matching_filters)
    };

    if !lane_supports_filter {
        // This lane cannot push a filter down into its own fetch — see the
        // module doc's "Which lanes can push a filter down" section.
        // Serving unfiltered would silently widen past what the grant
        // says, so this denies rather than approximates.
        return Ok(PolicyDecision::Deny);
    }

    Ok(PolicyDecision::Allow {
        filter: Some(effective_filter),
    })
}

/// Charges every rate ceiling (`#188`) that applies to a request
/// [`authorize_resource`] has already allowed, and reports whether they all
/// admit it. Call this once per served request, right after the
/// authorization checkpoint — see this module's own doc for why it is a
/// separate call rather than a third branch of `PolicyDecision`.
///
/// ## What "applies" means
///
/// Exactly the grants that authorized the request: held in the resource's
/// own tenant, naming this lane, covering this catalog/collection, and — for
/// a filtered grant — satisfied by this subject's claims. A grant whose
/// `{{claims.NAME}}` placeholder has no claim to substitute never authorized
/// anything (`authorize_resource`'s own rule), so its ceiling is never
/// charged either. Unlike `authorize_resource`, this never *parses* the
/// substituted filter: a template that substitutes but no longer parses has
/// already failed the authorization call with `Err`, so this function can
/// only ever run after that check passed.
///
/// Three cases charge nothing at all and return [`RateVerdict::Permitted`]
/// immediately, which is what makes "nothing configured, nothing changed"
/// literal here: `charge` is [`RateCharge::Skip`]; the subject is not a
/// member of the resource's tenant (a `public`/`shared_with` cross-tenant
/// read consults no grant at all, so there is no grant condition to charge —
/// bounding anonymous public traffic is admission control's job, not a
/// grant's); or the tenant's role table declares no ceiling anywhere.
///
/// ## Several ceilings compose conjunctively
///
/// Every applicable ceiling must admit the request, and every one of them is
/// charged. This is deliberately the opposite of how the same grants'
/// *filters* compose (any single unrestricted grant makes the whole decision
/// unrestricted): a filter is a permission, and holding a wider one honestly
/// grants more; a ceiling is a bound, and OR-ing bounds would let a subject
/// dodge a tight ceiling merely by also holding a looser grant that matched
/// the same request — which would make a tight ceiling unenforceable by
/// anyone who can be granted a second role. Charging every applicable
/// counter (rather than stopping at the first refusal) keeps each declared
/// ceiling's count meaning "requests this grant covered," independent of
/// which other grants happened to match alongside it.
///
/// When more than one ceiling refuses, the reported refusal is the one with
/// the LONGEST `retry_after_seconds`: with conjunctive bounds the client
/// cannot succeed until the last of them resets, and advertising the
/// earliest would invite a retry that is certain to be refused again.
pub async fn enforce_rate_limits(
    config: &AppConfig,
    resource: &ResourceContext<'_>,
    subject: &Subject,
    counter: Option<&dyn RateCounter>,
    charge: RateCharge,
) -> RateVerdict {
    if charge == RateCharge::Skip {
        return RateVerdict::Permitted;
    }
    // A cross-tenant read authorized by `public`/`shared_with` never
    // consulted a grant (see the module doc), so no grant condition applies.
    if !subject.is_member_of(resource.tenant_id) {
        return RateVerdict::Permitted;
    }
    let role_table = resolve_role_table(config, resource.tenant_id);
    if role_table.is_empty() {
        return RateVerdict::Permitted;
    }
    let held_roles = subject
        .memberships
        .get(resource.tenant_id)
        .cloned()
        .unwrap_or_default();

    let mut worst: Option<RateRefusal> = None;
    for role_name in &held_roles {
        let Some(grants) = role_table.get(role_name.as_str()) else {
            continue;
        };
        for grant in *grants {
            let Some(rate) = &grant.rate else {
                continue;
            };
            if !grant.lanes.contains(&resource.lane) {
                continue;
            }
            if !grant
                .scope
                .matches(resource.catalog_id, resource.collection_id)
            {
                continue;
            }
            // A filtered grant this subject cannot satisfy authorized
            // nothing, so it charges nothing — see this function's own doc.
            if let Some(template) = &grant.filter {
                if substitute_claims(template, &subject.claims).is_none() {
                    continue;
                }
            }
            let scope_value = scope_value_for(rate.scope, resource.tenant_id, role_name, subject);
            if let ConditionOutcome::Refused(refusal) =
                evaluate_condition(rate, resource.tenant_id, scope_value, counter).await
            {
                let longer = worst
                    .as_ref()
                    .is_none_or(|held| refusal.retry_after_seconds > held.retry_after_seconds);
                if longer {
                    worst = Some(refusal);
                }
            }
        }
    }

    match worst {
        Some(refusal) => RateVerdict::Refused(refusal),
        None => RateVerdict::Permitted,
    }
}

/// The value one condition's counter is keyed by, or `None` when this
/// subject carries no identity for that scope — see
/// `rate_limit`'s own module doc for how a `None` here is routed through the
/// condition's declared failure posture instead of being guessed at.
fn scope_value_for(
    scope: RateScope,
    tenant_id: &str,
    role_name: &str,
    subject: &Subject,
) -> Option<String> {
    match scope {
        RateScope::Principal => subject.principal.clone(),
        // The role name alone: `CounterKey` already carries the tenant, and
        // a role name is only ever meaningful within one tenant's table.
        RateScope::Role => Some(role_name.to_string()),
        RateScope::Tenant => Some(tenant_id.to_string()),
    }
}

/// Overlays `config.policy.roles` (platform-shared) with `tenant_id`'s own
/// tenant-custom document (`config.policy.tenant_policies`), if one exists —
/// a role name declared in both has the tenant-custom grants win outright,
/// per the module doc's "nearest wins, whole role replaces" rule. The
/// returned map borrows every `GrantDecl` from `config`, never clones one.
fn resolve_role_table<'a>(
    config: &'a AppConfig,
    tenant_id: &str,
) -> HashMap<&'a str, &'a Vec<GrantDecl>> {
    let mut table: HashMap<&str, &Vec<GrantDecl>> = config
        .policy
        .roles
        .iter()
        .map(|role: &'a RoleDecl| (role.name.as_str(), &role.grants))
        .collect();
    if let Some(tenant_policy) = config
        .policy
        .tenant_policies
        .iter()
        .find(|tp| tp.tenant == tenant_id)
    {
        for role in &tenant_policy.roles {
            table.insert(role.name.as_str(), &role.grants);
        }
    }
    table
}

/// Placeholder marker for ABAC claim substitution — see
/// `config::GrantDecl::filter`'s own doc. Duplicated from
/// `config.rs`'s own (config-validate-time, dummy-substituted) scan rather
/// than shared: the two operate on different inputs (a fixed dummy value
/// there, a real claims map here) and are each only a few lines — a shared
/// abstraction across the config-validate/runtime-evaluate boundary would
/// add more indirection than the ~15 duplicated lines save.
const CLAIM_PLACEHOLDER_PREFIX: &str = "{{claims.";
const CLAIM_PLACEHOLDER_SUFFIX: &str = "}}";

/// Substitutes every `{{claims.NAME}}` placeholder in `template` with the
/// CQL2-text literal for `claims[NAME]` — a quoted, quote-escaped string for
/// a JSON string claim, a bare number for a JSON number, `true`/`false` for
/// a JSON bool. Returns `None` (the "grant not satisfied" signal
/// `authorize_resource` acts on — see the module doc) when a referenced
/// claim is missing, or is a JSON array/object/null (no CQL2 literal shape
/// for those).
fn substitute_claims(
    template: &str,
    claims: &HashMap<String, serde_json::Value>,
) -> Option<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find(CLAIM_PLACEHOLDER_PREFIX) {
        let (before, after_prefix) = rest.split_at(start);
        out.push_str(before);
        let after_prefix = &after_prefix[CLAIM_PLACEHOLDER_PREFIX.len()..];
        let end = after_prefix.find(CLAIM_PLACEHOLDER_SUFFIX)?;
        let claim_name = &after_prefix[..end];
        let value = claims.get(claim_name)?;
        out.push_str(&cql2_literal_for_claim(value)?);
        rest = &after_prefix[end + CLAIM_PLACEHOLDER_SUFFIX.len()..];
    }
    out.push_str(rest);
    Some(out)
}

/// The CQL2-text literal spelling for one claim value — `None` for a shape
/// CQL2 has no scalar literal for (array, object, null).
fn cql2_literal_for_claim(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(format!("'{}'", s.replace('\'', "''"))),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) | serde_json::Value::Null => {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CatalogDecl, GrantScope, PolicyConfig, TenantPolicyDecl};
    use std::collections::HashSet;

    fn config_with(
        tenants: &[&str],
        policy: PolicyConfig,
        visibilities: &[(&str, &str, VisibilityDecl)],
    ) -> AppConfig {
        let mut config = AppConfig {
            tenants: tenants
                .iter()
                .map(|id| crate::config::TenantDecl {
                    id: id.to_string(),
                    external_id: None,
                    settings: Default::default(),
                })
                .collect(),
            policy,
            ..Default::default()
        };
        for (catalog_id, tenant_id, visibility) in visibilities {
            config.catalogs.push(CatalogDecl {
                id: catalog_id.to_string(),
                external_id: None,
                tenant: tenant_id.to_string(),
                settings: Default::default(),
                visibility: visibility.clone(),
            });
        }
        config
    }

    fn subject_member_of(tenant: &str, roles: &[&str]) -> Subject {
        let mut memberships = HashMap::new();
        memberships.insert(
            tenant.to_string(),
            roles.iter().map(|r| r.to_string()).collect(),
        );
        Subject {
            memberships,
            claims: HashMap::new(),
            principal: Some(format!("principal-of-{tenant}")),
            identity: None,
        }
    }

    fn resource<'a>(
        tenant_id: &'a str,
        catalog_id: &'a str,
        collection_id: &'a str,
        lane: PolicyLane,
        visibility: &'a VisibilityDecl,
    ) -> ResourceContext<'a> {
        ResourceContext {
            tenant_id,
            catalog_id,
            collection_id,
            lane,
            visibility,
        }
    }

    // -- nothing configured = unchanged (directive 10) -----------------------

    #[test]
    fn a_tenant_member_with_no_policy_configured_reads_unrestricted() {
        let config = config_with(&["tenant-a"], PolicyConfig::default(), &[]);
        let subject = subject_member_of("tenant-a", &[]);
        let visibility = VisibilityDecl::default();
        let decision = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::Allow { filter: None });
    }

    // -- isolation: member / non-member / anonymous / public / shared -------

    #[test]
    fn a_non_member_is_denied_a_private_resource() {
        let config = config_with(&["tenant-a", "tenant-b"], PolicyConfig::default(), &[]);
        let subject = subject_member_of("tenant-b", &[]);
        let visibility = VisibilityDecl::default();
        let decision = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::Deny);
    }

    #[test]
    fn an_anonymous_subject_is_denied_a_private_resource() {
        let config = config_with(&["tenant-a"], PolicyConfig::default(), &[]);
        let subject = Subject::anonymous();
        let visibility = VisibilityDecl::default();
        let decision = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::Deny);
    }

    #[test]
    fn an_anonymous_subject_reads_a_public_resource_unrestricted() {
        let config = config_with(&["tenant-a"], PolicyConfig::default(), &[]);
        let subject = Subject::anonymous();
        let visibility = VisibilityDecl {
            public: true,
            shared_with: vec![],
        };
        let decision = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::Allow { filter: None });
    }

    #[test]
    fn a_member_of_a_tenant_named_in_shared_with_reads_unrestricted() {
        let config = config_with(&["tenant-a", "tenant-b"], PolicyConfig::default(), &[]);
        let subject = subject_member_of("tenant-b", &[]);
        let visibility = VisibilityDecl {
            public: false,
            shared_with: vec!["tenant-b".to_string()],
        };
        let decision = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::Allow { filter: None });
    }

    #[test]
    fn a_member_of_an_unrelated_tenant_is_still_denied_when_shared_with_names_someone_else() {
        let config = config_with(
            &["tenant-a", "tenant-b", "tenant-c"],
            PolicyConfig::default(),
            &[],
        );
        let subject = subject_member_of("tenant-c", &[]);
        let visibility = VisibilityDecl {
            public: false,
            shared_with: vec!["tenant-b".to_string()],
        };
        let decision = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::Deny);
    }

    #[test]
    fn cross_tenant_reads_via_public_never_consult_rbac_and_stay_unrestricted() {
        // RBAC is active for tenant-a (a role is declared), but the reading
        // subject holds no role in tenant-a at all (they aren't even a
        // member) — a public resource must still read unrestricted, per the
        // module doc's "cross-tenant reads never consult RBAC" rule.
        let policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope::default(),
                    lanes: vec![PolicyLane::Features],
                    filter: None,
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        let config = config_with(&["tenant-a", "tenant-b"], policy, &[]);
        let subject = subject_member_of("tenant-b", &[]);
        let visibility = VisibilityDecl {
            public: true,
            shared_with: vec![],
        };
        let decision = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::Allow { filter: None });
    }

    // -- RBAC: role/grant matching -------------------------------------------

    #[test]
    fn rbac_active_denies_a_member_with_no_matching_role() {
        let policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope::default(),
                    lanes: vec![PolicyLane::Features],
                    filter: None,
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        let config = config_with(&["tenant-a"], policy, &[]);
        let subject = subject_member_of("tenant-a", &[]); // no roles held
        let visibility = VisibilityDecl::default();
        let decision = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::Deny);
    }

    #[test]
    fn rbac_active_allows_a_member_holding_a_matching_role() {
        let policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope::default(),
                    lanes: vec![PolicyLane::Features],
                    filter: None,
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        let config = config_with(&["tenant-a"], policy, &[]);
        let subject = subject_member_of("tenant-a", &["reader"]);
        let visibility = VisibilityDecl::default();
        let decision = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::Allow { filter: None });
    }

    #[test]
    fn a_grant_scoped_to_a_different_lane_does_not_match() {
        let policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope::default(),
                    lanes: vec![PolicyLane::Tiles],
                    filter: None,
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        let config = config_with(&["tenant-a"], policy, &[]);
        let subject = subject_member_of("tenant-a", &["reader"]);
        let visibility = VisibilityDecl::default();
        let decision = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::Deny);
    }

    // -- `#68`: PolicyLane::Write is never implied by a read grant ----------

    #[test]
    fn a_read_only_grant_never_authorizes_the_write_lane() {
        let policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope::default(),
                    lanes: vec![PolicyLane::Features],
                    filter: None,
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        let config = config_with(&["tenant-a"], policy, &[]);
        let subject = subject_member_of("tenant-a", &["reader"]);
        let visibility = VisibilityDecl::default();
        let decision = authorize_resource(
            &config,
            &resource("tenant-a", "cat-a", "col-a", PolicyLane::Write, &visibility),
            &subject,
            false,
        )
        .unwrap();
        assert_eq!(
            decision,
            PolicyDecision::Deny,
            "a read grant must never be implied to cover the write lane"
        );
    }

    #[test]
    fn a_grant_naming_the_write_lane_authorizes_a_write() {
        let policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "writer".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope::default(),
                    lanes: vec![PolicyLane::Write],
                    filter: None,
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        let config = config_with(&["tenant-a"], policy, &[]);
        let subject = subject_member_of("tenant-a", &["writer"]);
        let visibility = VisibilityDecl::default();
        let decision = authorize_resource(
            &config,
            &resource("tenant-a", "cat-a", "col-a", PolicyLane::Write, &visibility),
            &subject,
            false,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::Allow { filter: None });

        // The same subject still cannot read features through this grant —
        // write and read are independent, neither implies the other.
        let read_decision = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(read_decision, PolicyDecision::Deny);
    }

    #[test]
    fn a_grant_scoped_to_a_specific_collection_does_not_match_a_different_one() {
        let policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope {
                        catalogs: vec![],
                        collections: vec!["col-a".to_string()],
                    },
                    lanes: vec![PolicyLane::Features],
                    filter: None,
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        let config = config_with(&["tenant-a"], policy, &[]);
        let subject = subject_member_of("tenant-a", &["reader"]);
        let visibility = VisibilityDecl::default();
        let allowed = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(allowed, PolicyDecision::Allow { filter: None });

        let denied = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-b",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(denied, PolicyDecision::Deny);
    }

    // -- ABAC: filter compilation, claim substitution ------------------------

    #[test]
    fn a_filtered_grant_compiles_the_substituted_claim_into_the_filter() {
        let policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope::default(),
                    lanes: vec![PolicyLane::Features],
                    filter: Some("org = {{claims.org}}".to_string()),
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        let config = config_with(&["tenant-a"], policy, &[]);
        let mut subject = subject_member_of("tenant-a", &["reader"]);
        subject
            .claims
            .insert("org".to_string(), serde_json::json!("acme"));
        let visibility = VisibilityDecl::default();
        let decision = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        match decision {
            PolicyDecision::Allow {
                filter: Some(filter),
            } => {
                assert_eq!(
                    filter,
                    filter::parse_text("org = 'acme'").unwrap(),
                    "the grant's filter template must substitute the subject's real claim value"
                );
            }
            other => panic!("expected Allow with a filter, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_claim_makes_the_grant_unsatisfied_and_denies_when_no_other_grant_matches() {
        let policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope::default(),
                    lanes: vec![PolicyLane::Features],
                    filter: Some("org = {{claims.org}}".to_string()),
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        let config = config_with(&["tenant-a"], policy, &[]);
        // No `org` claim on this subject.
        let subject = subject_member_of("tenant-a", &["reader"]);
        let visibility = VisibilityDecl::default();
        let decision = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::Deny);
    }

    #[test]
    fn an_unconditional_grant_wins_over_a_filtered_one_held_via_a_different_role() {
        let policy = PolicyConfig {
            roles: vec![
                RoleDecl {
                    name: "filtered-reader".to_string(),
                    grants: vec![GrantDecl {
                        scope: GrantScope::default(),
                        lanes: vec![PolicyLane::Features],
                        filter: Some("org = {{claims.org}}".to_string()),
                        rate: None,
                    }],
                },
                RoleDecl {
                    name: "full-reader".to_string(),
                    grants: vec![GrantDecl {
                        scope: GrantScope::default(),
                        lanes: vec![PolicyLane::Features],
                        filter: None,
                        rate: None,
                    }],
                },
            ],
            tenant_policies: vec![],
        };
        let config = config_with(&["tenant-a"], policy, &[]);
        let subject = subject_member_of("tenant-a", &["filtered-reader", "full-reader"]);
        let visibility = VisibilityDecl::default();
        let decision = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        )
        .unwrap();
        assert_eq!(
            decision,
            PolicyDecision::Allow { filter: None },
            "holding an unconditional grant through any role must win over a filtered one"
        );
    }

    // -- lanes that cannot push a filter down --------------------------------

    #[test]
    fn a_filtered_grant_denies_on_a_lane_that_cannot_push_the_filter_down() {
        let policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope::default(),
                    lanes: vec![PolicyLane::Tiles],
                    filter: Some("org = {{claims.org}}".to_string()),
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        let config = config_with(&["tenant-a"], policy, &[]);
        let mut subject = subject_member_of("tenant-a", &["reader"]);
        subject
            .claims
            .insert("org".to_string(), serde_json::json!("acme"));
        let visibility = VisibilityDecl::default();
        let decision = authorize_resource(
            &config,
            &resource("tenant-a", "cat-a", "col-a", PolicyLane::Tiles, &visibility),
            &subject,
            false, // tiles: cannot push a filter down
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::Deny);
    }

    #[test]
    fn an_unconditional_grant_still_allows_on_a_lane_that_cannot_push_a_filter_down() {
        let policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope::default(),
                    lanes: vec![PolicyLane::Tiles],
                    filter: None,
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        let config = config_with(&["tenant-a"], policy, &[]);
        let subject = subject_member_of("tenant-a", &["reader"]);
        let visibility = VisibilityDecl::default();
        let decision = authorize_resource(
            &config,
            &resource("tenant-a", "cat-a", "col-a", PolicyLane::Tiles, &visibility),
            &subject,
            false,
        )
        .unwrap();
        assert_eq!(decision, PolicyDecision::Allow { filter: None });
    }

    // -- tenant-custom overlay: nearest wins, and cannot widen ---------------

    #[test]
    fn a_tenant_custom_role_replaces_the_platform_roles_grants_for_the_same_name() {
        let policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope::default(),
                    lanes: vec![PolicyLane::Features],
                    filter: None, // platform: unrestricted
                    rate: None,
                }],
            }],
            tenant_policies: vec![TenantPolicyDecl {
                tenant: "tenant-a".to_string(),
                roles: vec![RoleDecl {
                    name: "reader".to_string(),
                    grants: vec![GrantDecl {
                        scope: GrantScope::default(),
                        lanes: vec![PolicyLane::Features],
                        filter: Some("org = {{claims.org}}".to_string()), // tenant-a: narrowed
                        rate: None,
                    }],
                }],
            }],
        };
        let config = config_with(&["tenant-a", "tenant-b"], policy, &[]);

        // tenant-a's own tenant-custom document narrows "reader".
        let mut subject_a = subject_member_of("tenant-a", &["reader"]);
        subject_a
            .claims
            .insert("org".to_string(), serde_json::json!("acme"));
        let visibility = VisibilityDecl::default();
        let decision_a = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject_a,
            true,
        )
        .unwrap();
        assert!(
            matches!(decision_a, PolicyDecision::Allow { filter: Some(_) }),
            "tenant-a's own tenant-custom document must win outright for 'reader': {decision_a:?}"
        );

        // tenant-b has no tenant-custom document, so it still sees the
        // platform-shared (unrestricted) "reader" — a tenant policy narrows
        // only its own tenant, never another one's.
        let subject_b = subject_member_of("tenant-b", &["reader"]);
        let decision_b = authorize_resource(
            &config,
            &resource(
                "tenant-b",
                "cat-b",
                "col-b",
                PolicyLane::Features,
                &visibility,
            ),
            &subject_b,
            true,
        )
        .unwrap();
        assert_eq!(
            decision_b,
            PolicyDecision::Allow { filter: None },
            "tenant-a's narrowing must never leak into tenant-b's own role table"
        );
    }

    // -- misconfiguration surfaces as Err, not Deny --------------------------

    #[test]
    fn a_grant_filter_that_fails_to_parse_once_substituted_is_an_error_not_a_deny() {
        // `T_AFTER`'s second argument must be a `TIMESTAMP('...')` or a bare
        // string literal (`filter::Parser::expect_temporal_literal`) — this
        // grant's `since` claim is a JSON number, which substitutes as a
        // bare numeric token instead. Config-load time's own eager check
        // (`config::validate_grant_filter_template`) never catches this: it
        // always substitutes a *string* dummy literal for every placeholder
        // regardless of the claim's real declared type, so this specific
        // shape mismatch only surfaces once a real claim value is
        // substituted — exactly the defense-in-depth gap this function's
        // own doc describes. It must surface as `Err`, mapped by the caller
        // to a 500, distinct from a client-caused `Deny`.
        let policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope::default(),
                    lanes: vec![PolicyLane::Features],
                    filter: Some("T_AFTER(observed_at, {{claims.since}})".to_string()),
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        let config = config_with(&["tenant-a"], policy, &[]);
        let mut subject = subject_member_of("tenant-a", &["reader"]);
        subject
            .claims
            .insert("since".to_string(), serde_json::json!(5));
        let visibility = VisibilityDecl::default();
        let result = authorize_resource(
            &config,
            &resource(
                "tenant-a",
                "cat-a",
                "col-a",
                PolicyLane::Features,
                &visibility,
            ),
            &subject,
            true,
        );
        assert!(matches!(result, Err(Error::Config(_))));
    }

    // -- `#188`: rate ceilings as grant conditions ---------------------------

    use crate::rate_limit::{
        CounterPosture, CounterUnavailable, InProcessRateCounter, RateCounter, RateLimitDecl,
        RateObservation, RateRefusalCause, RateScope,
    };

    fn rate(scope: RateScope, ceiling: u64, posture: CounterPosture) -> RateLimitDecl {
        RateLimitDecl {
            scope,
            window_seconds: 60,
            ceiling,
            on_counter_unavailable: posture,
        }
    }

    fn policy_with_rate(rate: Option<RateLimitDecl>) -> PolicyConfig {
        PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope::default(),
                    lanes: vec![PolicyLane::Features],
                    filter: None,
                    rate,
                }],
            }],
            tenant_policies: vec![],
        }
    }

    /// Drives `enforce_rate_limits` `times` times against one fixed
    /// subject/resource, returning the verdict of the LAST call.
    async fn charge_n(
        config: &AppConfig,
        subject: &Subject,
        counter: Option<&dyn RateCounter>,
        times: usize,
    ) -> RateVerdict {
        let visibility = VisibilityDecl::default();
        let resource = resource(
            "tenant-a",
            "cat-a",
            "col-a",
            PolicyLane::Features,
            &visibility,
        );
        let mut verdict = RateVerdict::Permitted;
        for _ in 0..times {
            verdict =
                enforce_rate_limits(config, &resource, subject, counter, RateCharge::Charge).await;
        }
        verdict
    }

    #[tokio::test]
    async fn a_grant_declaring_no_ceiling_never_refuses_however_often_it_is_charged() {
        let config = config_with(&["tenant-a"], policy_with_rate(None), &[]);
        let subject = subject_member_of("tenant-a", &["reader"]);
        let counter = InProcessRateCounter::new();
        assert_eq!(
            charge_n(&config, &subject, Some(&counter), 50).await,
            RateVerdict::Permitted
        );
        assert_eq!(
            counter.tracked_keys(),
            0,
            "a grant with no rate condition must never even key a counter"
        );
    }

    #[tokio::test]
    async fn a_ceiling_admits_exactly_its_declared_count_then_refuses() {
        let config = config_with(
            &["tenant-a"],
            policy_with_rate(Some(rate(RateScope::Principal, 3, CounterPosture::Strict))),
            &[],
        );
        let subject = subject_member_of("tenant-a", &["reader"]);
        let counter = InProcessRateCounter::new();
        assert_eq!(
            charge_n(&config, &subject, Some(&counter), 3).await,
            RateVerdict::Permitted,
            "the third request of a ceiling of 3 is still inside it"
        );
        match charge_n(&config, &subject, Some(&counter), 1).await {
            RateVerdict::Refused(refusal) => {
                assert_eq!(refusal.cause, RateRefusalCause::CeilingReached);
                assert_eq!(refusal.ceiling, 3);
                assert!(refusal.retry_after_seconds >= 1);
            }
            RateVerdict::Permitted => panic!("the fourth request crosses a ceiling of 3"),
        }
    }

    #[tokio::test]
    async fn a_probe_never_charges_the_ceiling_it_would_otherwise_hit() {
        let config = config_with(
            &["tenant-a"],
            policy_with_rate(Some(rate(RateScope::Principal, 1, CounterPosture::Strict))),
            &[],
        );
        let subject = subject_member_of("tenant-a", &["reader"]);
        let counter = InProcessRateCounter::new();
        let visibility = VisibilityDecl::default();
        let resource = resource(
            "tenant-a",
            "cat-a",
            "col-a",
            PolicyLane::Features,
            &visibility,
        );
        for _ in 0..20 {
            assert_eq!(
                enforce_rate_limits(
                    &config,
                    &resource,
                    &subject,
                    Some(&counter),
                    RateCharge::Skip
                )
                .await,
                RateVerdict::Permitted
            );
        }
        assert_eq!(
            counter.tracked_keys(),
            0,
            "a listing's visibility probes must leave the caller's whole budget intact"
        );
        assert_eq!(
            charge_n(&config, &subject, Some(&counter), 1).await,
            RateVerdict::Permitted,
            "the one real request after 20 probes must still be inside a ceiling of 1"
        );
    }

    #[tokio::test]
    async fn two_principals_never_spend_each_others_budget() {
        let config = config_with(
            &["tenant-a"],
            policy_with_rate(Some(rate(RateScope::Principal, 1, CounterPosture::Strict))),
            &[],
        );
        let counter = InProcessRateCounter::new();
        let mut alice = subject_member_of("tenant-a", &["reader"]);
        alice.principal = Some("alice".to_string());
        let mut bob = subject_member_of("tenant-a", &["reader"]);
        bob.principal = Some("bob".to_string());

        assert_eq!(
            charge_n(&config, &alice, Some(&counter), 1).await,
            RateVerdict::Permitted
        );
        assert!(matches!(
            charge_n(&config, &alice, Some(&counter), 1).await,
            RateVerdict::Refused(_)
        ));
        assert_eq!(
            charge_n(&config, &bob, Some(&counter), 1).await,
            RateVerdict::Permitted,
            "alice exhausting her own ceiling must not touch bob's"
        );
    }

    #[tokio::test]
    async fn a_tenant_scoped_ceiling_is_shared_across_principals() {
        let config = config_with(
            &["tenant-a"],
            policy_with_rate(Some(rate(RateScope::Tenant, 1, CounterPosture::Strict))),
            &[],
        );
        let counter = InProcessRateCounter::new();
        let mut alice = subject_member_of("tenant-a", &["reader"]);
        alice.principal = Some("alice".to_string());
        let mut bob = subject_member_of("tenant-a", &["reader"]);
        bob.principal = Some("bob".to_string());

        assert_eq!(
            charge_n(&config, &alice, Some(&counter), 1).await,
            RateVerdict::Permitted
        );
        assert!(
            matches!(
                charge_n(&config, &bob, Some(&counter), 1).await,
                RateVerdict::Refused(_)
            ),
            "a tenant-scoped ceiling is one budget for every member, by definition"
        );
    }

    #[tokio::test]
    async fn a_principal_scoped_ceiling_in_one_tenant_never_throttles_the_other() {
        // The same platform-shared role, held in both tenants by the same
        // principal: two independent budgets, one per tenant.
        let config = config_with(
            &["tenant-a", "tenant-b"],
            policy_with_rate(Some(rate(RateScope::Principal, 1, CounterPosture::Strict))),
            &[],
        );
        let counter = InProcessRateCounter::new();
        let mut subject = Subject {
            memberships: HashMap::new(),
            claims: HashMap::new(),
            principal: Some("alice".to_string()),
            identity: None,
        };
        for tenant in ["tenant-a", "tenant-b"] {
            subject
                .memberships
                .insert(tenant.to_string(), HashSet::from(["reader".to_string()]));
        }
        let visibility = VisibilityDecl::default();
        let in_b = resource(
            "tenant-b",
            "cat-b",
            "col-b",
            PolicyLane::Features,
            &visibility,
        );

        assert_eq!(
            charge_n(&config, &subject, Some(&counter), 1).await,
            RateVerdict::Permitted
        );
        assert!(matches!(
            charge_n(&config, &subject, Some(&counter), 1).await,
            RateVerdict::Refused(_)
        ));
        assert_eq!(
            enforce_rate_limits(&config, &in_b, &subject, Some(&counter), RateCharge::Charge).await,
            RateVerdict::Permitted,
            "one tenant's document must never consume the same principal's budget elsewhere"
        );
    }

    #[tokio::test]
    async fn a_grant_for_a_different_lane_charges_nothing() {
        let mut policy =
            policy_with_rate(Some(rate(RateScope::Principal, 1, CounterPosture::Strict)));
        policy.roles[0].grants[0].lanes = vec![PolicyLane::Stac];
        let config = config_with(&["tenant-a"], policy, &[]);
        let subject = subject_member_of("tenant-a", &["reader"]);
        let counter = InProcessRateCounter::new();
        assert_eq!(
            charge_n(&config, &subject, Some(&counter), 10).await,
            RateVerdict::Permitted
        );
        assert_eq!(counter.tracked_keys(), 0);
    }

    #[tokio::test]
    async fn a_grant_whose_claim_is_missing_charges_nothing() {
        // The same rule `authorize_resource` applies: a filtered grant this
        // subject cannot satisfy authorized nothing, so its ceiling is not
        // this subject's to spend.
        let mut policy =
            policy_with_rate(Some(rate(RateScope::Principal, 1, CounterPosture::Strict)));
        policy.roles[0].grants[0].filter = Some("org = {{claims.org}}".to_string());
        let config = config_with(&["tenant-a"], policy, &[]);
        let subject = subject_member_of("tenant-a", &["reader"]); // no `org` claim
        let counter = InProcessRateCounter::new();
        assert_eq!(
            charge_n(&config, &subject, Some(&counter), 10).await,
            RateVerdict::Permitted
        );
        assert_eq!(counter.tracked_keys(), 0);
    }

    #[tokio::test]
    async fn the_tightest_of_two_matching_ceilings_is_the_one_that_binds() {
        // Conjunctive composition: holding a second, looser grant must never
        // be a way to dodge a tight ceiling — see `enforce_rate_limits`.
        let policy = PolicyConfig {
            roles: vec![
                RoleDecl {
                    name: "tight".to_string(),
                    grants: vec![GrantDecl {
                        scope: GrantScope::default(),
                        lanes: vec![PolicyLane::Features],
                        filter: None,
                        rate: Some(rate(RateScope::Principal, 1, CounterPosture::Strict)),
                    }],
                },
                RoleDecl {
                    name: "loose".to_string(),
                    grants: vec![GrantDecl {
                        scope: GrantScope::default(),
                        lanes: vec![PolicyLane::Features],
                        filter: None,
                        rate: Some(rate(RateScope::Principal, 1_000, CounterPosture::Strict)),
                    }],
                },
            ],
            tenant_policies: vec![],
        };
        let config = config_with(&["tenant-a"], policy, &[]);
        let subject = subject_member_of("tenant-a", &["tight", "loose"]);
        let counter = InProcessRateCounter::new();
        assert_eq!(
            charge_n(&config, &subject, Some(&counter), 1).await,
            RateVerdict::Permitted
        );
        assert!(
            matches!(
                charge_n(&config, &subject, Some(&counter), 1).await,
                RateVerdict::Refused(_)
            ),
            "the looser grant must not dilute the tighter one"
        );
        assert_eq!(
            counter.tracked_keys(),
            2,
            "each declared ceiling keeps its own count, whichever one refused"
        );
    }

    #[tokio::test]
    async fn a_cross_tenant_public_read_charges_no_grant_condition() {
        let config = config_with(
            &["tenant-a", "tenant-b"],
            policy_with_rate(Some(rate(RateScope::Tenant, 1, CounterPosture::Strict))),
            &[],
        );
        let subject = subject_member_of("tenant-b", &["reader"]);
        let counter = InProcessRateCounter::new();
        let visibility = VisibilityDecl {
            public: true,
            shared_with: vec![],
        };
        let resource = resource(
            "tenant-a",
            "cat-a",
            "col-a",
            PolicyLane::Features,
            &visibility,
        );
        for _ in 0..10 {
            assert_eq!(
                enforce_rate_limits(
                    &config,
                    &resource,
                    &subject,
                    Some(&counter),
                    RateCharge::Charge
                )
                .await,
                RateVerdict::Permitted,
                "a public read consults no grant, so it has no grant condition to charge"
            );
        }
        assert_eq!(counter.tracked_keys(), 0);
    }

    #[tokio::test]
    async fn an_inactive_role_table_charges_nothing() {
        let config = config_with(&["tenant-a"], PolicyConfig::default(), &[]);
        let subject = subject_member_of("tenant-a", &[]);
        let counter = InProcessRateCounter::new();
        assert_eq!(
            charge_n(&config, &subject, Some(&counter), 10).await,
            RateVerdict::Permitted
        );
        assert_eq!(counter.tracked_keys(), 0);
    }

    struct BrokenCounter;

    #[async_trait::async_trait]
    impl RateCounter for BrokenCounter {
        async fn observe(
            &self,
            _key: &crate::rate_limit::CounterKey,
        ) -> std::result::Result<RateObservation, CounterUnavailable> {
            Err(CounterUnavailable {
                reason: "the test counter is deliberately broken",
            })
        }
    }

    #[tokio::test]
    async fn a_broken_counter_refuses_under_strict_and_serves_under_graceful() {
        let subject = subject_member_of("tenant-a", &["reader"]);

        let strict = config_with(
            &["tenant-a"],
            policy_with_rate(Some(rate(
                RateScope::Principal,
                100,
                CounterPosture::Strict,
            ))),
            &[],
        );
        match charge_n(&strict, &subject, Some(&BrokenCounter), 1).await {
            RateVerdict::Refused(refusal) => {
                assert_eq!(refusal.cause, RateRefusalCause::CounterUnavailable);
                assert_eq!(refusal.retry_after_seconds, 60);
            }
            RateVerdict::Permitted => panic!("a strict posture must refuse an unevaluable bound"),
        }

        let graceful = config_with(
            &["tenant-a"],
            policy_with_rate(Some(rate(
                RateScope::Principal,
                100,
                CounterPosture::Graceful,
            ))),
            &[],
        );
        assert_eq!(
            charge_n(&graceful, &subject, Some(&BrokenCounter), 1).await,
            RateVerdict::Permitted
        );
    }

    #[tokio::test]
    async fn a_subject_with_no_principal_takes_the_declared_posture() {
        let mut subject = subject_member_of("tenant-a", &["reader"]);
        subject.principal = None;
        let counter = InProcessRateCounter::new();

        let strict = config_with(
            &["tenant-a"],
            policy_with_rate(Some(rate(
                RateScope::Principal,
                100,
                CounterPosture::Strict,
            ))),
            &[],
        );
        assert!(matches!(
            charge_n(&strict, &subject, Some(&counter), 1).await,
            RateVerdict::Refused(_)
        ));

        let graceful = config_with(
            &["tenant-a"],
            policy_with_rate(Some(rate(
                RateScope::Principal,
                100,
                CounterPosture::Graceful,
            ))),
            &[],
        );
        assert_eq!(
            charge_n(&graceful, &subject, Some(&counter), 1).await,
            RateVerdict::Permitted
        );

        // ... while a tenant-scoped ceiling is perfectly keyable for the
        // very same subject, and still enforces normally.
        let by_tenant = config_with(
            &["tenant-a"],
            policy_with_rate(Some(rate(RateScope::Tenant, 1, CounterPosture::Strict))),
            &[],
        );
        assert_eq!(
            charge_n(&by_tenant, &subject, Some(&counter), 1).await,
            RateVerdict::Permitted
        );
        assert!(matches!(
            charge_n(&by_tenant, &subject, Some(&counter), 1).await,
            RateVerdict::Refused(_)
        ));
    }

    #[tokio::test]
    async fn no_counter_backend_at_all_takes_the_declared_posture() {
        let subject = subject_member_of("tenant-a", &["reader"]);
        let strict = config_with(
            &["tenant-a"],
            policy_with_rate(Some(rate(
                RateScope::Principal,
                100,
                CounterPosture::Strict,
            ))),
            &[],
        );
        assert!(matches!(
            charge_n(&strict, &subject, None, 1).await,
            RateVerdict::Refused(_)
        ));

        let graceful = config_with(
            &["tenant-a"],
            policy_with_rate(Some(rate(
                RateScope::Principal,
                100,
                CounterPosture::Graceful,
            ))),
            &[],
        );
        assert_eq!(
            charge_n(&graceful, &subject, None, 1).await,
            RateVerdict::Permitted
        );
    }

    // -- HashSet import sanity ------------------------------------------------
    #[test]
    fn subject_member_of_builds_the_expected_role_set() {
        let subject = subject_member_of("tenant-a", &["a", "b"]);
        assert_eq!(
            subject.memberships.get("tenant-a").cloned().unwrap(),
            HashSet::from(["a".to_string(), "b".to_string()])
        );
    }
}
