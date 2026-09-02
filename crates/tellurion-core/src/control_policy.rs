//! Hierarchical, path-scoped administration policy (`#215`).
//!
//! This module answers one question and nothing else: **given an
//! authenticated principal, an HTTP method and a canonical administrative
//! path, may this request proceed?** It never authenticates, never sees a
//! credential, and never touches an HTTP type — the same framework-free
//! placement `auth::TenantAuthorizer` documents for itself, for the same
//! reason.
//!
//! # The precedence rule
//!
//! Stated once here, as a rule, and implemented below as a property of the
//! *set* of statements in effect rather than of the order they are visited
//! in — so no reordering of bindings or policies can change an answer.
//!
//! For a request with method `M`, canonical path `CP` and resolved target
//! scope `S` — where `CP` is the request path decoded exactly as axum
//! decodes it (`control_path::decoded_segments`) with every external id
//! replaced by its internal one, which is what makes an encoded separator,
//! a dot segment or an alias unable to produce a `CP` other than the one
//! belonging to the resource the handler will actually serve:
//!
//! **R0 — engagement.** A declared statement *mentions* `CP` when one of
//! its patterns matches it. If no statement in the active snapshot mentions
//! `CP`, this module returns [`ControlDecision::NotEngaged`] and the request
//! is decided by exactly the gates that decided it before `#215` existed.
//! A deployment that declares no statements is therefore unchanged on every
//! path, and a deployment that declares statements about one subtree is
//! unchanged everywhere else. Engagement deliberately ignores `M`: writing
//! a statement about a path brings *every* method on that path under
//! default-deny, because the opposite reading ("only the methods you named
//! are governed") lets an operator who restricted `GET` leave `DELETE` wide
//! open without ever being told.
//!
//! **R1 — composition.** When engaged, this decision is an *additional*
//! gate applied after the tenant / platform-admin trust boundary, never
//! instead of it. It can only narrow: it never turns a refusal into an
//! acceptance, so no statement can grant reach that the pre-`#215` server
//! did not already permit.
//!
//! **R2 — downward inheritance, never upward.** A role binding at scope `B`
//! contributes its role to this request only when `B` covers `S`, where
//! `platform` covers every scope, `tenant/t` covers `t` and everything
//! under it, `catalog/t/c` covers `c` and its collections, and a collection
//! scope covers only itself ([`ControlScope::covers`]). Authority flows
//! down the hierarchy and never up: a binding at a catalog never authorises
//! its parent tenant or the platform, and never a sibling catalog — which
//! is what makes "nested resources cannot be authorized under the wrong
//! parent" true by construction rather than by a check someone has to
//! remember to write.
//!
//! **R3 — a statement is in effect** iff (a) some contributing role names
//! it, (b) its `methods` contain `M`, (c) one of its patterns matches `CP`,
//! and (d) it carries no [`PolicyCondition`] of a kind this build does not
//! implement — where clause (d) applies to `Allow` statements only. This
//! build implements *no* condition kinds, so a conditioned `Allow` can
//! never grant and a conditioned `Deny` always denies. Fail-closed in both
//! directions, and named: see [`ControlPolicySet::unhonoured_conditions`],
//! which the boot/reload path reports so a condition nobody evaluates is
//! never silently treated as satisfied.
//!
//! **R4 — precedence.** If any in-effect statement is `Deny`, the decision
//! is `Deny`. Otherwise, if any is `Allow`, the decision is `Allow`.
//! Otherwise the decision is `Deny`. Explicit deny beats allow **in both
//! directions of depth**: a deny bound deeper overrides an allow bound
//! shallower, and a deny bound shallower overrides an allow bound deeper.
//! Depth breaks no ties; effect does. Absence of an allow is a deny.
//!
//! # What a decision may say
//!
//! [`ControlDecisionContext`] names the scope, the statements that were in
//! effect and the roles that contributed them. It exists for the audit
//! trail and for a future policy simulator — never for a response body. A
//! refusal that named the statement that produced it would be a policy
//! oracle: an unauthorised caller could enumerate the policy document by
//! probing, which is the same exposure `#208` refused when it kept the
//! `Allow` header subject-independent. The server layer therefore renders
//! every refusal from a fixed string and reads this context only into
//! records the refused caller cannot see.
//!
//! # What this module does not decide
//!
//! Built-in role *templates* (`sysadmin`, `tenant_admin`, …) carry no
//! implicit permissions here. A role is a name; it grants exactly what a
//! declared statement says it grants, and nothing else. Shipping a built-in
//! grant table would be an invented default of precisely the kind this
//! campaign forbids — an operator who wrote `tenant_admin` would receive
//! reach they never wrote down. The one pre-existing exception, `sysadmin`
//! at platform scope resolving to platform-admin authority (`#239`,
//! `auth::build_authorizer_with_bindings`), is untouched by this module.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::control_model::{
    ControlScope, PathPolicy, PolicyEffect, PrincipalIdentity, RoleBinding,
};
use crate::control_path::PathPattern;
use crate::error::Result;

/// The four administrative resource shapes this server serves today, keyed
/// by the canonical segment list a request canonicalized to.
///
/// A closed table on purpose. Policy must never be able to attach itself to
/// a path this table does not name, and a data-plane path (`/{t}/features/
/// …`) must never be mistaken for an administrative one — so recognition is
/// exhaustive matching on a fixed set of shapes, not a prefix test.
///
/// The captured ids here are the EXTERNAL ones the caller typed. They are
/// replaced by internal ids before any pattern is matched
/// ([`AdminResource::resolve`]), which is what stops one resource reachable
/// under two external ids from having two different policy answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminResource {
    /// `/config` — the raw config document (`#110`).
    PlatformConfig,
    /// `/config/effective` — the platform effective-settings view.
    PlatformEffective,
    /// `/config/profiles` — named settings profiles (`#111`).
    PlatformProfiles,
    /// `/config/webhooks` — the webhook subscription list.
    PlatformWebhooks,
    /// `/config/webhooks/{subscription}/dead-letters`.
    PlatformWebhookDeadLetters { subscription: String },
    /// `/{tenant}/config/effective`.
    TenantEffective { tenant: String },
    /// `/{tenant}/config/catalogs/{catalog}/effective`.
    CatalogEffective { tenant: String, catalog: String },
    /// `/{tenant}/config/catalogs/{catalog}/collections/{collection}/effective`.
    CollectionEffective {
        tenant: String,
        catalog: String,
        collection: String,
    },
}

impl AdminResource {
    /// Which administrative resource `segments` names, or `None` for any
    /// path that is not one — including every data-plane path, which is why
    /// the checkpoint can be applied broadly and still touch nothing it does
    /// not own.
    ///
    /// `segments` must already be decoded
    /// ([`crate::control_path::decoded_segments`]) — the same single decode
    /// axum applies, so this classifies the resource the handler will
    /// actually serve rather than a differently-decoded neighbour of it.
    pub fn of(segments: &[String]) -> Option<Self> {
        let parts: Vec<&str> = segments.iter().map(String::as_str).collect();
        match parts.as_slice() {
            ["config"] => Some(Self::PlatformConfig),
            ["config", "effective"] => Some(Self::PlatformEffective),
            ["config", "profiles"] => Some(Self::PlatformProfiles),
            ["config", "webhooks"] => Some(Self::PlatformWebhooks),
            ["config", "webhooks", subscription, "dead-letters"] => {
                Some(Self::PlatformWebhookDeadLetters {
                    subscription: (*subscription).to_string(),
                })
            }
            [tenant, "config", "effective"] => Some(Self::TenantEffective {
                tenant: (*tenant).to_string(),
            }),
            [tenant, "config", "catalogs", catalog, "effective"] => Some(Self::CatalogEffective {
                tenant: (*tenant).to_string(),
                catalog: (*catalog).to_string(),
            }),
            [tenant, "config", "catalogs", catalog, "collections", collection, "effective"] => {
                Some(Self::CollectionEffective {
                    tenant: (*tenant).to_string(),
                    catalog: (*catalog).to_string(),
                    collection: (*collection).to_string(),
                })
            }
            _ => None,
        }
    }

    /// The external ids this resource named, in hierarchy order. `None` at a
    /// level this resource does not reach.
    pub fn external_ids(&self) -> (Option<&str>, Option<&str>, Option<&str>) {
        match self {
            Self::PlatformConfig
            | Self::PlatformEffective
            | Self::PlatformProfiles
            | Self::PlatformWebhooks
            | Self::PlatformWebhookDeadLetters { .. } => (None, None, None),
            Self::TenantEffective { tenant } => (Some(tenant), None, None),
            Self::CatalogEffective { tenant, catalog } => (Some(tenant), Some(catalog), None),
            Self::CollectionEffective {
                tenant,
                catalog,
                collection,
            } => (Some(tenant), Some(catalog), Some(collection)),
        }
    }

    /// The canonical request this resource becomes once its external ids
    /// have been resolved to internal ones and its ownership verified.
    ///
    /// The caller supplies the internal ids for exactly the levels
    /// [`external_ids`](Self::external_ids) reported, having resolved each
    /// one *within its parent* — that resolution is the ownership check, and
    /// it is the caller's because it is I/O.
    ///
    /// Panics only on a caller that supplies ids for the wrong levels, which
    /// is a programming error rather than a request-shaped one.
    pub fn resolve(
        &self,
        method: &str,
        tenant_id: Option<&str>,
        catalog_id: Option<&str>,
        collection_id: Option<&str>,
    ) -> ControlRequestContext {
        let literal = |segments: &[&str]| segments.iter().map(|s| (*s).to_string()).collect();
        let (canonical_path, scope) = match self {
            Self::PlatformConfig => (literal(&["config"]), ControlScope::Platform),
            Self::PlatformEffective => (literal(&["config", "effective"]), ControlScope::Platform),
            Self::PlatformProfiles => (literal(&["config", "profiles"]), ControlScope::Platform),
            Self::PlatformWebhooks => (literal(&["config", "webhooks"]), ControlScope::Platform),
            Self::PlatformWebhookDeadLetters { subscription } => (
                vec![
                    "config".to_string(),
                    "webhooks".to_string(),
                    subscription.clone(),
                    "dead-letters".to_string(),
                ],
                ControlScope::Platform,
            ),
            Self::TenantEffective { .. } => {
                let tenant = tenant_id.expect("a tenant resource resolves a tenant id");
                (
                    vec![
                        tenant.to_string(),
                        "config".to_string(),
                        "effective".to_string(),
                    ],
                    ControlScope::Tenant {
                        tenant_id: tenant.to_string(),
                    },
                )
            }
            Self::CatalogEffective { .. } => {
                let tenant = tenant_id.expect("a catalog resource resolves a tenant id");
                let catalog = catalog_id.expect("a catalog resource resolves a catalog id");
                (
                    vec![
                        tenant.to_string(),
                        "config".to_string(),
                        "catalogs".to_string(),
                        catalog.to_string(),
                        "effective".to_string(),
                    ],
                    ControlScope::Catalog {
                        tenant_id: tenant.to_string(),
                        catalog_id: catalog.to_string(),
                    },
                )
            }
            Self::CollectionEffective { .. } => {
                let tenant = tenant_id.expect("a collection resource resolves a tenant id");
                let catalog = catalog_id.expect("a collection resource resolves a catalog id");
                let collection =
                    collection_id.expect("a collection resource resolves a collection id");
                (
                    vec![
                        tenant.to_string(),
                        "config".to_string(),
                        "catalogs".to_string(),
                        catalog.to_string(),
                        "collections".to_string(),
                        collection.to_string(),
                        "effective".to_string(),
                    ],
                    ControlScope::Collection {
                        tenant_id: tenant.to_string(),
                        catalog_id: catalog.to_string(),
                        collection_id: collection.to_string(),
                    },
                )
            }
        };
        ControlRequestContext {
            method: method.to_ascii_uppercase(),
            canonical_path,
            scope,
        }
    }
}

/// One administrative request, canonicalized and resolved — the whole input
/// to [`ControlPolicySet::authorize`] besides the principal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlRequestContext {
    /// Uppercase HTTP method.
    pub method: String,
    /// Canonical segments, every external id already replaced by its
    /// internal one.
    pub canonical_path: Vec<String>,
    /// The typed scope this request targets.
    pub scope: ControlScope,
}

impl ControlRequestContext {
    /// The canonical path as a single absolute string — for audit records
    /// and diagnostics; matching always uses the segment list.
    pub fn canonical_path_string(&self) -> String {
        format!("/{}", self.canonical_path.join("/"))
    }
}

/// Why a decision came out the way it did (`#215` acceptance criterion: a
/// simulator explains allow/deny without exposing secrets). Contains
/// statement ids, role names and a scope key — declared policy vocabulary,
/// never a credential, never a token fingerprint, never a claim value.
///
/// Never rendered into a response body: see this module's own doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlDecisionContext {
    /// The target scope's resource key (`ControlScope::resource_key`).
    pub scope: String,
    /// Ids of the statements that were in effect, sorted.
    pub statements: Vec<String>,
    /// The role names that contributed those statements, sorted.
    pub roles: Vec<String>,
    pub basis: DecisionBasis,
}

impl ControlDecisionContext {
    /// A compact, single-line rendering for the audit trail.
    pub fn summary(&self) -> String {
        format!(
            "scope={} basis={} statements=[{}] roles=[{}]",
            self.scope,
            self.basis.as_str(),
            self.statements.join(","),
            self.roles.join(",")
        )
    }
}

/// Which clause of the precedence rule produced the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionBasis {
    /// R4, first clause: an in-effect `Deny` statement.
    ExplicitDeny,
    /// R4, second clause: an in-effect `Allow` statement and no `Deny`.
    ExplicitAllow,
    /// R4, third clause: the path is governed, but nothing this principal
    /// holds allows it.
    NoMatchingAllow,
}

impl DecisionBasis {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitDeny => "explicit_deny",
            Self::ExplicitAllow => "explicit_allow",
            Self::NoMatchingAllow => "no_matching_allow",
        }
    }
}

/// The outcome of [`ControlPolicySet::authorize`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlDecision {
    /// R0: no declared statement mentions this path, so this checkpoint has
    /// no opinion and the request keeps whatever answer it had before
    /// `#215`. Distinct from `Allow` on purpose — an `Allow` would claim
    /// this module authorised something, which for an un-governed path it
    /// did not.
    NotEngaged,
    Allow(ControlDecisionContext),
    Deny(ControlDecisionContext),
}

/// One role held at one scope by one principal.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BoundRole {
    scope: ControlScope,
    role: String,
}

/// A [`PathPolicy`] with its patterns compiled and its methods/roles indexed.
#[derive(Debug, Clone)]
struct CompiledStatement {
    id: String,
    effect: PolicyEffect,
    methods: HashSet<String>,
    patterns: Vec<PathPattern>,
    roles: HashSet<String>,
    /// R3(d): this statement declares at least one condition of a kind this
    /// build does not implement, so it may deny but may never allow.
    unhonoured_conditions: bool,
}

impl CompiledStatement {
    fn mentions(&self, path: &[String]) -> bool {
        self.patterns.iter().any(|pattern| pattern.matches(path))
    }
}

/// Every declared binding and statement, compiled once per activation.
///
/// [`Default`] is the empty set — no bindings, no statements — which by R0
/// means every path is `NotEngaged`. That is what makes an existing
/// deployment provably unchanged: it has no way to declare either, so it
/// gets this value, and this value has exactly one possible answer.
#[derive(Debug, Default)]
pub struct ControlPolicySet {
    bindings: HashMap<PrincipalIdentity, Vec<BoundRole>>,
    statements: Vec<CompiledStatement>,
    unhonoured_conditions: Vec<String>,
    roleless: Vec<String>,
}

impl ControlPolicySet {
    /// Compiles one activation's bindings and statements, or names the
    /// declaration that cannot be compiled.
    ///
    /// Compilation is where every pattern is parsed, so no request ever pays
    /// for parsing and no request can be shaped by a pattern that failed to
    /// parse: a bad pattern fails the whole activation, leaving the previous
    /// policy set serving, exactly as a bad `token_env` already fails one
    /// (`#144`).
    pub fn compile(bindings: &[RoleBinding], policies: &[PathPolicy]) -> Result<Self> {
        let mut by_principal: HashMap<PrincipalIdentity, Vec<BoundRole>> = HashMap::new();
        for binding in bindings {
            by_principal
                .entry(binding.principal.clone())
                .or_default()
                .push(BoundRole {
                    scope: binding.scope.clone(),
                    role: binding.role.clone(),
                });
        }

        let mut statements = Vec::with_capacity(policies.len());
        let mut unhonoured_conditions = Vec::new();
        let mut roleless = Vec::new();
        for policy in policies {
            let mut patterns = Vec::with_capacity(policy.patterns.len());
            for pattern in &policy.patterns {
                patterns.push(PathPattern::compile(pattern)?);
            }
            // No condition kind is implemented by this build; every declared
            // condition is therefore unhonoured. Recorded by policy id so
            // the boot path can say so out loud rather than behaving as if
            // the condition had been evaluated and passed.
            let unhonoured = !policy.conditions.is_empty();
            if unhonoured {
                unhonoured_conditions.push(policy.id.clone());
            }
            let checkpoint_roles = if policy.roles.is_empty() {
                policy.role.iter().cloned().collect::<Vec<_>>()
            } else {
                policy.roles.clone()
            };
            if checkpoint_roles.is_empty() {
                roleless.push(policy.id.clone());
            }
            statements.push(CompiledStatement {
                id: policy.id.clone(),
                effect: policy.effect,
                methods: policy
                    .methods
                    .iter()
                    .map(|method| method.to_ascii_uppercase())
                    .collect(),
                patterns,
                roles: checkpoint_roles.into_iter().collect(),
                unhonoured_conditions: unhonoured,
            });
        }
        unhonoured_conditions.sort();
        roleless.sort();
        Ok(Self {
            bindings: by_principal,
            statements,
            unhonoured_conditions,
            roleless,
        })
    }

    /// No statements at all — every path is `NotEngaged` (R0). The predicate
    /// the boot path checks before saying anything about policy, so a
    /// deployment that declared none never sees a line about a subsystem it
    /// is not using.
    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }

    pub fn statement_count(&self) -> usize {
        self.statements.len()
    }

    pub fn binding_count(&self) -> usize {
        self.bindings.values().map(Vec::len).sum()
    }

    /// Statement ids declaring a [`PolicyCondition`](crate::PolicyCondition)
    /// of a kind this build does not implement (R3d). Reported at activation
    /// so an operator learns that such a statement cannot grant — never
    /// silently treated as satisfied, and never silently dropped either.
    pub fn unhonoured_conditions(&self) -> &[String] {
        &self.unhonoured_conditions
    }

    /// Statement ids naming no role at all. Such a statement is reachable by
    /// nobody, so it can neither allow nor deny — but by R0 it still brings
    /// its paths under default-deny, which is a large enough consequence to
    /// be worth naming rather than leaving an operator to discover.
    pub fn roleless_statements(&self) -> &[String] {
        &self.roleless
    }

    /// The precedence rule, applied. See this module's own doc; the code
    /// below is deliberately a transcription of R0–R4 in order.
    ///
    /// `identity` is the authenticated principal, or `None` for a request
    /// that established none. `None` holds no bindings, therefore
    /// contributes no roles, therefore reaches R4's third clause on any
    /// engaged path — default-deny, with no special case written for it.
    pub fn authorize(
        &self,
        identity: Option<&PrincipalIdentity>,
        request: &ControlRequestContext,
    ) -> ControlDecision {
        // R0: engagement is a property of the path alone.
        let mentioned: Vec<&CompiledStatement> = self
            .statements
            .iter()
            .filter(|statement| statement.mentions(&request.canonical_path))
            .collect();
        if mentioned.is_empty() {
            return ControlDecision::NotEngaged;
        }

        // R2: only bindings whose scope covers the target scope contribute.
        let contributing: BTreeSet<&str> = identity
            .and_then(|identity| self.bindings.get(identity))
            .map(|bound| {
                bound
                    .iter()
                    .filter(|bound| bound.scope.covers(&request.scope))
                    .map(|bound| bound.role.as_str())
                    .collect()
            })
            .unwrap_or_default();

        // R3: which of the mentioning statements are actually in effect.
        let mut denies: BTreeSet<&str> = BTreeSet::new();
        let mut allows: BTreeSet<&str> = BTreeSet::new();
        for statement in mentioned {
            if !statement.methods.contains(&request.method) {
                continue;
            }
            if !statement
                .roles
                .iter()
                .any(|role| contributing.contains(role.as_str()))
            {
                continue;
            }
            match statement.effect {
                PolicyEffect::Deny => {
                    denies.insert(statement.id.as_str());
                }
                PolicyEffect::Allow if statement.unhonoured_conditions => {}
                PolicyEffect::Allow => {
                    allows.insert(statement.id.as_str());
                }
            }
        }

        // R4: set semantics, so iteration order above cannot matter.
        let roles: Vec<String> = contributing
            .iter()
            .map(|role| (*role).to_string())
            .collect();
        let scope = request.scope.resource_key();
        if !denies.is_empty() {
            return ControlDecision::Deny(ControlDecisionContext {
                scope,
                statements: denies.iter().map(|id| (*id).to_string()).collect(),
                roles,
                basis: DecisionBasis::ExplicitDeny,
            });
        }
        if !allows.is_empty() {
            return ControlDecision::Allow(ControlDecisionContext {
                scope,
                statements: allows.iter().map(|id| (*id).to_string()).collect(),
                roles,
                basis: DecisionBasis::ExplicitAllow,
            });
        }
        ControlDecision::Deny(ControlDecisionContext {
            scope,
            statements: Vec::new(),
            roles,
            basis: DecisionBasis::NoMatchingAllow,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_model::PolicyCondition;

    fn principal(subject: &str) -> PrincipalIdentity {
        PrincipalIdentity {
            issuer: "urn:test".to_string(),
            subject: subject.to_string(),
        }
    }

    fn binding(subject: &str, role: &str, scope: ControlScope) -> RoleBinding {
        RoleBinding {
            principal: principal(subject),
            role: role.to_string(),
            scope,
        }
    }

    fn statement(id: &str, effect: PolicyEffect, roles: &[&str], patterns: &[&str]) -> PathPolicy {
        PathPolicy {
            id: id.to_string(),
            role: None,
            scope: None,
            effect,
            methods: vec!["GET".to_string()],
            patterns: patterns.iter().map(|p| (*p).to_string()).collect(),
            roles: roles.iter().map(|r| (*r).to_string()).collect(),
            conditions: Vec::new(),
        }
    }

    fn request(path: &[&str], scope: ControlScope) -> ControlRequestContext {
        ControlRequestContext {
            method: "GET".to_string(),
            canonical_path: path.iter().map(|s| (*s).to_string()).collect(),
            scope,
        }
    }

    fn tenant(id: &str) -> ControlScope {
        ControlScope::Tenant {
            tenant_id: id.to_string(),
        }
    }

    fn catalog(tenant_id: &str, catalog_id: &str) -> ControlScope {
        ControlScope::Catalog {
            tenant_id: tenant_id.to_string(),
            catalog_id: catalog_id.to_string(),
        }
    }

    fn allowed(decision: &ControlDecision) -> bool {
        matches!(decision, ControlDecision::Allow(_))
    }

    /// R0, the rule an existing deployment depends on.
    #[test]
    fn an_empty_policy_set_is_never_engaged() {
        let set = ControlPolicySet::default();
        assert_eq!(
            set.authorize(
                Some(&principal("a")),
                &request(&["acme", "config", "effective"], tenant("acme"))
            ),
            ControlDecision::NotEngaged
        );
        assert!(set.is_empty());
    }

    /// R0 again: a statement about one subtree leaves every other path
    /// exactly as it was.
    #[test]
    fn a_statement_engages_only_the_paths_it_mentions() {
        let set = ControlPolicySet::compile(
            &[],
            &[statement(
                "acme-only",
                PolicyEffect::Allow,
                &["viewer"],
                &["/acme/config/**"],
            )],
        )
        .unwrap();
        assert_eq!(
            set.authorize(None, &request(&["config"], ControlScope::Platform)),
            ControlDecision::NotEngaged
        );
        assert!(matches!(
            set.authorize(
                None,
                &request(&["acme", "config", "effective"], tenant("acme"))
            ),
            ControlDecision::Deny(_)
        ));
    }

    /// R4, both directions of depth, in one place. Each case names the pair
    /// of scopes whose statements conflict.
    #[test]
    fn explicit_deny_beats_allow_in_both_directions_of_depth() {
        let policies = [
            statement("allow-all", PolicyEffect::Allow, &["reader"], &["/**"]),
            statement("deny-all", PolicyEffect::Deny, &["blocked"], &["/**"]),
        ];
        // platform allow vs tenant deny, judged at tenant scope.
        let set = ControlPolicySet::compile(
            &[
                binding("p", "reader", ControlScope::Platform),
                binding("p", "blocked", tenant("acme")),
            ],
            &policies,
        )
        .unwrap();
        assert!(matches!(
            set.authorize(
                Some(&principal("p")),
                &request(&["acme", "config", "effective"], tenant("acme"))
            ),
            ControlDecision::Deny(_)
        ));

        // tenant deny vs catalog allow, judged at catalog scope: the
        // shallower deny still wins.
        let set = ControlPolicySet::compile(
            &[
                binding("p", "blocked", tenant("acme")),
                binding("p", "reader", catalog("acme", "cadastre")),
            ],
            &policies,
        )
        .unwrap();
        assert!(matches!(
            set.authorize(
                Some(&principal("p")),
                &request(
                    &["acme", "config", "catalogs", "cadastre", "effective"],
                    catalog("acme", "cadastre")
                )
            ),
            ControlDecision::Deny(_)
        ));
    }

    /// R4 is a property of the set, not of iteration order.
    #[test]
    fn the_decision_does_not_depend_on_declaration_order() {
        let bindings = [
            binding("p", "reader", ControlScope::Platform),
            binding("p", "blocked", tenant("acme")),
        ];
        let allow = statement("allow-all", PolicyEffect::Allow, &["reader"], &["/**"]);
        let deny = statement("deny-all", PolicyEffect::Deny, &["blocked"], &["/**"]);
        let target = request(&["acme", "config", "effective"], tenant("acme"));

        let forward = ControlPolicySet::compile(&bindings, &[allow.clone(), deny.clone()])
            .unwrap()
            .authorize(Some(&principal("p")), &target);
        let reversed = ControlPolicySet::compile(&bindings, &[deny, allow])
            .unwrap()
            .authorize(Some(&principal("p")), &target);
        assert_eq!(forward, reversed);
        assert!(matches!(forward, ControlDecision::Deny(_)));
    }

    /// R2: down, never up, never sideways.
    #[test]
    fn authority_flows_down_the_hierarchy_and_never_up_or_sideways() {
        let policies = [statement(
            "read-everything",
            PolicyEffect::Allow,
            &["reader"],
            &["/**"],
        )];
        let set = ControlPolicySet::compile(
            &[binding("p", "reader", catalog("acme", "cadastre"))],
            &policies,
        )
        .unwrap();
        let subject = principal("p");

        // Down: the collection under the bound catalog.
        assert!(allowed(&set.authorize(
            Some(&subject),
            &request(
                &[
                    "acme",
                    "config",
                    "catalogs",
                    "cadastre",
                    "collections",
                    "parcels",
                    "effective"
                ],
                ControlScope::Collection {
                    tenant_id: "acme".to_string(),
                    catalog_id: "cadastre".to_string(),
                    collection_id: "parcels".to_string(),
                }
            )
        )));
        // Up: the parent tenant.
        assert!(!allowed(&set.authorize(
            Some(&subject),
            &request(&["acme", "config", "effective"], tenant("acme"))
        )));
        // Sideways: a sibling catalog in the same tenant.
        assert!(!allowed(&set.authorize(
            Some(&subject),
            &request(
                &["acme", "config", "catalogs", "zoning", "effective"],
                catalog("acme", "zoning")
            )
        )));
        // Sideways: the same catalog id under a different tenant.
        assert!(!allowed(&set.authorize(
            Some(&subject),
            &request(
                &["beta", "config", "catalogs", "cadastre", "effective"],
                catalog("beta", "cadastre")
            )
        )));
    }

    /// R3(d), fail-closed in both directions.
    #[test]
    fn an_unhonoured_condition_blocks_an_allow_but_never_a_deny() {
        let mut conditioned_allow = statement(
            "conditional-allow",
            PolicyEffect::Allow,
            &["reader"],
            &["/**"],
        );
        conditioned_allow.conditions = vec![PolicyCondition {
            kind: "not-implemented-by-this-build".to_string(),
            config: serde_json::Value::Null,
        }];
        let mut conditioned_deny = statement(
            "conditional-deny",
            PolicyEffect::Deny,
            &["reader"],
            &["/**"],
        );
        conditioned_deny.id = "conditional-deny".to_string();
        conditioned_deny.conditions = conditioned_allow.conditions.clone();

        let bindings = [binding("p", "reader", ControlScope::Platform)];
        let target = request(&["config", "effective"], ControlScope::Platform);

        let allow_only =
            ControlPolicySet::compile(&bindings, &[conditioned_allow.clone()]).unwrap();
        assert_eq!(
            allow_only.unhonoured_conditions(),
            ["conditional-allow".to_string()]
        );
        assert!(matches!(
            allow_only.authorize(Some(&principal("p")), &target),
            ControlDecision::Deny(_)
        ));

        let deny_only = ControlPolicySet::compile(&bindings, &[conditioned_deny]).unwrap();
        assert!(matches!(
            deny_only.authorize(Some(&principal("p")), &target),
            ControlDecision::Deny(_)
        ));
    }

    /// A role with no statement, and a statement with no role, both reach
    /// R4's third clause rather than accidentally granting.
    #[test]
    fn a_role_grants_exactly_what_a_statement_says_and_nothing_by_name() {
        let set = ControlPolicySet::compile(
            &[binding("p", "sysadmin", ControlScope::Platform)],
            &[statement(
                "no-roles",
                PolicyEffect::Allow,
                &[],
                &["/config/**"],
            )],
        )
        .unwrap();
        assert_eq!(set.roleless_statements(), ["no-roles".to_string()]);
        assert!(matches!(
            set.authorize(
                Some(&principal("p")),
                &request(&["config", "effective"], ControlScope::Platform)
            ),
            ControlDecision::Deny(_)
        ));
    }

    #[test]
    fn a_method_the_statement_does_not_name_is_not_allowed_by_it() {
        let set = ControlPolicySet::compile(
            &[binding("p", "reader", ControlScope::Platform)],
            &[statement(
                "read-only",
                PolicyEffect::Allow,
                &["reader"],
                &["/config"],
            )],
        )
        .unwrap();
        let mut write = request(&["config"], ControlScope::Platform);
        write.method = "PUT".to_string();
        assert!(matches!(
            set.authorize(Some(&principal("p")), &write),
            ControlDecision::Deny(_)
        ));
        assert!(allowed(&set.authorize(
            Some(&principal("p")),
            &request(&["config"], ControlScope::Platform)
        )));
    }

    #[test]
    fn the_administrative_route_table_is_closed() {
        let segments = |path: &str| crate::control_path::decoded_segments(path).unwrap();
        assert_eq!(
            AdminResource::of(&segments("/config")),
            Some(AdminResource::PlatformConfig)
        );
        assert_eq!(
            AdminResource::of(&segments("/acme/config/catalogs/c/effective")),
            Some(AdminResource::CatalogEffective {
                tenant: "acme".to_string(),
                catalog: "c".to_string()
            })
        );
        // Data-plane paths are not administrative resources and are never
        // governed by this checkpoint.
        assert_eq!(
            AdminResource::of(&segments("/acme/features/catalogs/c/collections/x/items")),
            None
        );
        assert_eq!(AdminResource::of(&segments("/metrics")), None);
        assert_eq!(AdminResource::of(&segments("/acme/config")), None);
    }

    /// The canonical path is built from internal ids, so two external ids
    /// for one resource cannot receive two different answers.
    #[test]
    fn the_canonical_path_is_built_from_internal_ids() {
        let resource = AdminResource::of(&[
            "public-alias".to_string(),
            "config".to_string(),
            "effective".to_string(),
        ])
        .unwrap();
        let resolved = resource.resolve("get", Some("acme"), None, None);
        assert_eq!(resolved.canonical_path_string(), "/acme/config/effective");
        assert_eq!(resolved.method, "GET");
        assert_eq!(resolved.scope, tenant("acme"));
    }
}
