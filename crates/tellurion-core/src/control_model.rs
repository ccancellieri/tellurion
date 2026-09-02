//! Backend-neutral domain types for the dynamic control plane.
//!
//! These types contain no database-specific values. Concrete stores persist
//! the same validated snapshots, revisions, audit records, and ordered change
//! events regardless of backend.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, CatalogDecl, CollectionDecl, TenantDecl};
use crate::error::{Error, Result};

pub type ControlRevision = u64;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrincipalIdentity {
    pub issuer: String,
    pub subject: String,
}

impl PrincipalIdentity {
    pub(crate) fn validate(&self, context: &str) -> Result<()> {
        if self.issuer.trim().is_empty() {
            return Err(Error::ControlValidation(format!(
                "{context}: principal issuer must not be empty"
            )));
        }
        if self.subject.trim().is_empty() {
            return Err(Error::ControlValidation(format!(
                "{context}: principal subject must not be empty"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlScope {
    Platform,
    Tenant {
        tenant_id: String,
    },
    Catalog {
        tenant_id: String,
        catalog_id: String,
    },
    Collection {
        tenant_id: String,
        catalog_id: String,
        collection_id: String,
    },
}

impl ControlScope {
    /// Compatibility name used by the request checkpoint: authority flows
    /// from an ancestor scope to the same scope or one of its descendants.
    pub fn covers(&self, other: &Self) -> bool {
        self.contains(other)
    }

    /// Whether this scope is the same as, or an ancestor of, `candidate`.
    pub fn contains(&self, candidate: &Self) -> bool {
        match (self, candidate) {
            (Self::Platform, _) => true,
            (
                Self::Tenant { tenant_id },
                Self::Tenant {
                    tenant_id: candidate_tenant,
                }
                | Self::Catalog {
                    tenant_id: candidate_tenant,
                    ..
                }
                | Self::Collection {
                    tenant_id: candidate_tenant,
                    ..
                },
            ) => tenant_id == candidate_tenant,
            (
                Self::Catalog {
                    tenant_id,
                    catalog_id,
                },
                Self::Catalog {
                    tenant_id: candidate_tenant,
                    catalog_id: candidate_catalog,
                }
                | Self::Collection {
                    tenant_id: candidate_tenant,
                    catalog_id: candidate_catalog,
                    ..
                },
            ) => tenant_id == candidate_tenant && catalog_id == candidate_catalog,
            (
                Self::Collection {
                    tenant_id,
                    catalog_id,
                    collection_id,
                },
                Self::Collection {
                    tenant_id: candidate_tenant,
                    catalog_id: candidate_catalog,
                    collection_id: candidate_collection,
                },
            ) => {
                tenant_id == candidate_tenant
                    && catalog_id == candidate_catalog
                    && collection_id == candidate_collection
            }
            _ => false,
        }
    }

    pub fn resource_key(&self) -> String {
        match self {
            Self::Platform => "platform".to_string(),
            Self::Tenant { tenant_id } => format!("tenant/{tenant_id}"),
            Self::Catalog {
                tenant_id,
                catalog_id,
            } => format!("tenant/{tenant_id}/catalog/{catalog_id}"),
            Self::Collection {
                tenant_id,
                catalog_id,
                collection_id,
            } => format!("tenant/{tenant_id}/catalog/{catalog_id}/collection/{collection_id}"),
        }
    }

    pub fn validate_against(&self, config: &AppConfig) -> Result<()> {
        let tenant = |tenant_id: &str| {
            config
                .tenants
                .iter()
                .find(|tenant| tenant.id == tenant_id)
                .ok_or_else(|| Error::ControlValidation(format!("unknown tenant '{tenant_id}'")))
        };
        let catalog = |catalog_id: &str| {
            config
                .catalogs
                .iter()
                .find(|catalog| catalog.id == catalog_id)
                .ok_or_else(|| Error::ControlValidation(format!("unknown catalog '{catalog_id}'")))
        };

        match self {
            Self::Platform => Ok(()),
            Self::Tenant { tenant_id } => tenant(tenant_id).map(|_| ()),
            Self::Catalog {
                tenant_id,
                catalog_id,
            } => {
                tenant(tenant_id)?;
                let catalog = catalog(catalog_id)?;
                if catalog.tenant != *tenant_id {
                    return Err(Error::ControlValidation(format!(
                        "catalog '{catalog_id}' belongs to tenant '{}', not '{tenant_id}'",
                        catalog.tenant
                    )));
                }
                Ok(())
            }
            Self::Collection {
                tenant_id,
                catalog_id,
                collection_id,
            } => {
                tenant(tenant_id)?;
                let catalog = catalog(catalog_id)?;
                if catalog.tenant != *tenant_id {
                    return Err(Error::ControlValidation(format!(
                        "catalog '{catalog_id}' belongs to tenant '{}', not '{tenant_id}'",
                        catalog.tenant
                    )));
                }
                let collection = config
                    .collections
                    .iter()
                    .find(|collection| collection.id == *collection_id)
                    .ok_or_else(|| {
                        Error::ControlValidation(format!("unknown collection '{collection_id}'"))
                    })?;
                if collection.catalog != *catalog_id {
                    return Err(Error::ControlValidation(format!(
                        "collection '{collection_id}' belongs to catalog '{}', not '{catalog_id}'",
                        collection.catalog
                    )));
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RoleBinding {
    pub principal: PrincipalIdentity,
    pub role: String,
    pub scope: ControlScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyCondition {
    pub kind: String,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathPolicy {
    pub id: String,
    /// The persisted role bundle this statement belongs to. `None` is read
    /// only for snapshots written before role bundles were introduced and
    /// never grants authority.
    #[serde(default)]
    pub role: Option<String>,
    /// The effective scope of this statement. Kept optional solely so old
    /// snapshots deserialize; an unscoped statement never grants authority.
    #[serde(default)]
    pub scope: Option<ControlScope>,
    pub effect: PolicyEffect,
    pub methods: Vec<String>,
    pub patterns: Vec<String>,
    /// Roles consumed by the server request checkpoint. Scoped dynamic
    /// policies mirror their single `role` here so both policy engines
    /// enforce the same persisted statement.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    #[serde(default)]
    pub conditions: Vec<PolicyCondition>,
}

impl PathPolicy {
    pub fn new<M, P>(
        id: impl Into<String>,
        role: impl Into<String>,
        scope: ControlScope,
        effect: PolicyEffect,
        methods: impl IntoIterator<Item = M>,
        patterns: impl IntoIterator<Item = P>,
    ) -> Self
    where
        M: Into<String>,
        P: Into<String>,
    {
        let role = role.into();
        Self {
            id: id.into(),
            role: Some(role.clone()),
            scope: Some(scope),
            effect,
            methods: methods.into_iter().map(Into::into).collect(),
            patterns: patterns.into_iter().map(Into::into).collect(),
            roles: vec![role],
            conditions: Vec::new(),
        }
    }

    pub fn legacy<M, P>(
        id: impl Into<String>,
        effect: PolicyEffect,
        methods: impl IntoIterator<Item = M>,
        patterns: impl IntoIterator<Item = P>,
        conditions: Vec<PolicyCondition>,
    ) -> Self
    where
        M: Into<String>,
        P: Into<String>,
    {
        Self {
            id: id.into(),
            role: None,
            scope: None,
            effect,
            methods: methods.into_iter().map(Into::into).collect(),
            patterns: patterns.into_iter().map(Into::into).collect(),
            roles: Vec::new(),
            conditions,
        }
    }

    pub(crate) fn validate(&self) -> Result<Vec<crate::control_admin_path::CompiledPathPattern>> {
        if self.id.trim().is_empty() {
            return Err(Error::ControlValidation(
                "path policy id must not be empty".to_string(),
            ));
        }
        if self.methods.is_empty() {
            return Err(Error::ControlValidation(format!(
                "path policy '{}': methods must not be empty",
                self.id
            )));
        }
        if self.patterns.is_empty() {
            return Err(Error::ControlValidation(format!(
                "path policy '{}': patterns must not be empty",
                self.id
            )));
        }
        for method in &self.methods {
            if method.is_empty()
                || !method
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte == b'-')
            {
                return Err(Error::ControlValidation(format!(
                    "path policy '{}': method '{method}' must be an uppercase HTTP method",
                    self.id
                )));
            }
        }
        if self
            .role
            .as_ref()
            .is_some_and(|role| role.trim().is_empty())
        {
            return Err(Error::ControlValidation(format!(
                "path policy '{}': role must not be empty",
                self.id
            )));
        }
        for role in &self.roles {
            if role.trim().is_empty() {
                return Err(Error::ControlValidation(format!(
                    "path policy '{}': role names must not be empty",
                    self.id
                )));
            }
        }
        if let Some(role) = &self.role {
            if !self.roles.is_empty() && (self.roles.len() != 1 || self.roles.first() != Some(role))
            {
                return Err(Error::ControlValidation(format!(
                    "path policy '{}': role representations disagree",
                    self.id
                )));
            }
        }
        if self.role.is_some() != self.scope.is_some() {
            return Err(Error::ControlValidation(format!(
                "path policy '{}': role and scope must either both be present or both be absent",
                self.id
            )));
        }
        let compiled_patterns = if self.role.is_none() {
            for pattern in &self.patterns {
                crate::control_admin_path::validate_inert_legacy_pattern(pattern).map_err(
                    |_| {
                        Error::ControlValidation(format!(
                        "path policy '{}': pattern '{pattern}' must be an anchored segment pattern",
                        self.id
                    ))
                    },
                )?;
            }
            Vec::new()
        } else {
            self.patterns
                .iter()
                .map(|pattern| {
                    crate::control_admin_path::CompiledPathPattern::compile(pattern).map_err(|_| {
                        Error::ControlValidation(format!(
                            "path policy '{}': pattern '{pattern}' must be an anchored segment pattern",
                            self.id
                        ))
                    })
                })
                .collect::<Result<Vec<_>>>()?
        };
        for pattern in &self.patterns {
            crate::control_path::PathPattern::compile(pattern).map_err(|error| {
                Error::ControlValidation(format!("path policy '{}': {error}", self.id))
            })?;
        }
        for condition in &self.conditions {
            if condition.kind.trim().is_empty() {
                return Err(Error::ControlValidation(format!(
                    "path policy '{}': condition kind must not be empty",
                    self.id
                )));
            }
        }
        Ok(compiled_patterns)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlSnapshot {
    pub config: AppConfig,
    pub role_bindings: Vec<RoleBinding>,
    pub path_policies: Vec<PathPolicy>,
    #[serde(default)]
    pub tombstoned_resources: Vec<ControlScope>,
}

pub(crate) fn validate_durable_bearer_token_sources(config: &AppConfig) -> Result<()> {
    if config
        .auth
        .bearer_tokens
        .iter()
        .any(|entry| !entry.token.is_empty())
    {
        return Err(Error::ControlValidation(
            "durable control stores require auth.bearer_tokens entries to use token_env; inline token values are supported only by legacy file configuration"
                .to_string(),
        ));
    }
    Ok(())
}

impl ControlSnapshot {
    pub fn validate(&self) -> Result<()> {
        self.config.validate()?;
        validate_durable_bearer_token_sources(&self.config)?;
        crate::control_admin_policy::validate_builtin_policy_templates()?;

        let mut policy_ids = HashSet::new();
        for policy in &self.path_policies {
            let _ = policy.validate()?;
            if let Some(scope) = &policy.scope {
                scope.validate_against(&self.config)?;
                if !crate::control_admin_policy::policy_within_scope(policy, &self.config) {
                    return Err(Error::ControlValidation(format!(
                        "path policy '{}': pattern is outside its effective scope",
                        policy.id
                    )));
                }
            }
            if !policy_ids.insert(policy.id.as_str()) {
                return Err(Error::ControlValidation(format!(
                    "duplicate path policy id '{}'",
                    policy.id
                )));
            }
        }

        let mut bindings = HashSet::new();
        for binding in &self.role_bindings {
            binding.principal.validate("role binding")?;
            if binding.role.trim().is_empty() {
                return Err(Error::ControlValidation(
                    "role binding role must not be empty".to_string(),
                ));
            }
            binding.scope.validate_against(&self.config)?;
            if !bindings.insert(binding) {
                return Err(Error::ControlValidation(format!(
                    "duplicate role binding for role '{}' at '{}'",
                    binding.role,
                    binding.scope.resource_key()
                )));
            }
        }

        let mut tombstones = HashSet::new();
        for scope in &self.tombstoned_resources {
            scope.validate_against(&self.config)?;
            if !tombstones.insert(scope) {
                return Err(Error::ControlValidation(format!(
                    "duplicate tombstone for '{}'",
                    scope.resource_key()
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionedControlSnapshot {
    pub snapshot: ControlSnapshot,
    pub revision: ControlRevision,
    pub entity_versions: BTreeMap<String, String>,
    #[serde(skip)]
    validated: Option<crate::control_admin_policy::ValidatedControlSnapshot>,
    #[serde(skip)]
    validated_revision: Option<ControlRevision>,
    #[serde(skip)]
    validated_entity_versions: Option<BTreeMap<String, String>>,
}

impl VersionedControlSnapshot {
    pub fn new(
        snapshot: ControlSnapshot,
        revision: ControlRevision,
        entity_versions: BTreeMap<String, String>,
    ) -> Result<Self> {
        let validated = snapshot.validated()?;
        Ok(Self {
            snapshot,
            revision,
            entity_versions: entity_versions.clone(),
            validated: Some(validated),
            validated_revision: Some(revision),
            validated_entity_versions: Some(entity_versions),
        })
    }

    pub fn validated_snapshot(
        &self,
    ) -> Result<&crate::control_admin_policy::ValidatedControlSnapshot> {
        self.validated_state().map(|(validated, _, _)| validated)
    }

    pub(crate) fn validated_state(
        &self,
    ) -> Result<(
        &crate::control_admin_policy::ValidatedControlSnapshot,
        ControlRevision,
        &BTreeMap<String, String>,
    )> {
        let invalid = || {
            Error::ControlValidation(
                "versioned control snapshot was not constructed through validation".to_string(),
            )
        };
        Ok((
            self.validated.as_ref().ok_or_else(&invalid)?,
            self.validated_revision.ok_or_else(&invalid)?,
            self.validated_entity_versions
                .as_ref()
                .ok_or_else(invalid)?,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlChangeSet {
    pub idempotency_key: Option<String>,
    pub operations: Vec<VersionedControlOperation>,
}

impl ControlChangeSet {
    pub fn validate(&self) -> Result<()> {
        if self.operations.is_empty() {
            return Err(Error::ControlValidation(
                "control changeset must contain at least one operation".to_string(),
            ));
        }
        if self
            .idempotency_key
            .as_ref()
            .is_some_and(|key| key.trim().is_empty())
        {
            return Err(Error::ControlValidation(
                "idempotency key must not be empty".to_string(),
            ));
        }
        for operation in &self.operations {
            if operation
                .expected_entity_version
                .as_ref()
                .is_some_and(|version| version.trim().is_empty())
            {
                return Err(Error::ControlValidation(
                    "expected entity version must not be empty".to_string(),
                ));
            }
            if let ControlOperation::PutPathPolicy(policy) = &operation.operation {
                let _ = policy.validate()?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlCommit {
    pub revision: ControlRevision,
    pub changed_resources: Vec<String>,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlEvent {
    pub revision: ControlRevision,
    pub ordinal: u32,
    pub changed_resources: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ControlEventCursor {
    pub revision: ControlRevision,
    pub ordinal: u32,
}

impl ControlEvent {
    pub fn cursor(&self) -> ControlEventCursor {
        ControlEventCursor {
            revision: self.revision,
            ordinal: self.ordinal,
        }
    }
}

pub fn validate_control_event_page(
    after: Option<ControlEventCursor>,
    events: &[ControlEvent],
) -> Result<()> {
    let mut previous = after.unwrap_or(ControlEventCursor {
        revision: 0,
        ordinal: 0,
    });
    for event in events {
        let current = event.cursor();
        if current <= previous {
            return Err(Error::ControlEventOrder {
                previous_revision: previous.revision,
                previous_ordinal: previous.ordinal,
                revision: event.revision,
                ordinal: event.ordinal,
            });
        }
        previous = current;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRequestContext {
    pub method: String,
    pub canonical_path: String,
    pub correlation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BootstrapOutcome {
    Bootstrapped(ControlRevision),
    AlreadyInitialized(ControlRevision),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionedControlOperation {
    pub expected_entity_version: Option<String>,
    pub operation: ControlOperation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ControlOperation {
    ReplacePlatformSettings(AppConfig),
    PutTenant(TenantDecl),
    PutCatalog(CatalogDecl),
    PutCollection(CollectionDecl),
    TombstoneResource {
        scope: ControlScope,
    },
    PermanentlyDeleteResource {
        scope: ControlScope,
    },
    PutRoleBinding(RoleBinding),
    DeleteRoleBinding {
        principal: PrincipalIdentity,
        scope: ControlScope,
        role: String,
    },
    PutPathPolicy(PathPolicy),
    DeletePathPolicy {
        id: String,
    },
}

/// The validated, backend-neutral result of applying one control changeset.
///
/// Concrete stores persist this result atomically with their revision, audit,
/// outbox, and idempotency records. Keeping mutation semantics here prevents
/// SQLite, PostgreSQL, and test stores from implementing subtly different
/// resource-key or entity-version rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedControlChangeSet {
    pub snapshot: ControlSnapshot,
    pub entity_versions: BTreeMap<String, String>,
    pub changed_resources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControlPreview {
    pub base_revision: ControlRevision,
    pub prospective_revision: ControlRevision,
    pub changed_resources: Vec<String>,
    pub entity_versions: BTreeMap<String, String>,
    prospective_snapshot: ControlSnapshot,
}

impl ControlPreview {
    pub fn prospective_snapshot(&self) -> &ControlSnapshot {
        &self.prospective_snapshot
    }
}

pub fn preview_control_changes(
    snapshot: &VersionedControlSnapshot,
    authorization: &crate::AuthorizedControlMutation,
    changes: &ControlChangeSet,
) -> Result<ControlPreview> {
    let prospective_revision = snapshot.revision.checked_add(1).ok_or_else(|| {
        Error::ControlValidation("control revision cannot advance past u64::MAX".to_string())
    })?;
    let applied = apply_control_changes(
        snapshot.snapshot.clone(),
        snapshot.entity_versions.clone(),
        prospective_revision,
        authorization,
        changes,
    )?;
    let entity_versions = applied
        .changed_resources
        .iter()
        .filter_map(|resource| {
            applied
                .entity_versions
                .get(resource)
                .cloned()
                .map(|version| (resource.clone(), version))
        })
        .collect();
    Ok(ControlPreview {
        base_revision: snapshot.revision,
        prospective_revision,
        changed_resources: applied.changed_resources,
        entity_versions,
        prospective_snapshot: applied.snapshot,
    })
}

pub fn apply_control_changes(
    mut snapshot: ControlSnapshot,
    mut entity_versions: BTreeMap<String, String>,
    revision: ControlRevision,
    authorization: &crate::AuthorizedControlMutation,
    changes: &ControlChangeSet,
) -> Result<AppliedControlChangeSet> {
    changes.validate()?;
    authorization.validate_intent(changes)?;
    let expected_revision = revision
        .checked_sub(1)
        .ok_or_else(|| Error::ControlValidation("control revision must be positive".to_string()))?;
    if authorization.snapshot_revision() != expected_revision {
        return Err(Error::ControlRevisionConflict {
            expected: authorization.snapshot_revision(),
            current: expected_revision,
        });
    }
    snapshot.validate()?;
    authorization.validate_authoritative_state(&snapshot, &entity_versions)?;
    let authority_snapshot = snapshot.clone();
    let mut changed = BTreeSet::new();
    let mut version_targets = BTreeSet::new();
    let mut obsolete_versions = BTreeSet::new();
    let mut touched_catalogs = BTreeSet::new();
    let mut touched_collections = BTreeSet::new();
    for versioned in &changes.operations {
        if let ControlOperation::PermanentlyDeleteResource { scope } = &versioned.operation {
            if !authority_snapshot.tombstoned_resources.contains(scope) {
                return Err(Error::ControlValidation(format!(
                    "resource '{}' must be authoritatively tombstoned before permanent deletion",
                    scope.resource_key()
                )));
            }
            if versioned.expected_entity_version.is_none() {
                return Err(Error::ControlValidation(format!(
                    "permanent deletion of '{}' requires an expected entity version",
                    scope.resource_key()
                )));
            }
        }
        for operation_scope in
            operation_scopes(&versioned.operation, &authority_snapshot, &snapshot)?
        {
            if !authorization.effective_scope().contains(&operation_scope) {
                return Err(Error::ControlValidation(format!(
                    "authorized scope '{}' does not contain operation scope '{}'",
                    authorization.effective_scope().resource_key(),
                    operation_scope.resource_key(),
                )));
            }
        }
        let key = operation_expected_key(&versioned.operation, &authority_snapshot, &snapshot)?;
        if let Some(expected) = &versioned.expected_entity_version {
            let current = entity_versions
                .get(&key)
                .cloned()
                .unwrap_or_else(|| "0".to_string());
            if current != *expected {
                return Err(Error::ControlEntityVersionConflict {
                    resource: key,
                    expected: expected.clone(),
                    current,
                });
            }
        }
        if let ControlOperation::PutCollection(collection) = &versioned.operation {
            migrate_collection_control_state(&mut snapshot, collection)?;
        }
        apply_operation(&mut snapshot, &versioned.operation)?;
        match &versioned.operation {
            ControlOperation::PutCatalog(catalog) => {
                touched_catalogs.insert(catalog.id.clone());
            }
            ControlOperation::PutCollection(collection) => {
                touched_collections.insert(collection.id.clone());
            }
            ControlOperation::PermanentlyDeleteResource { .. } => {
                changed.insert(key.clone());
                obsolete_versions.insert(key);
            }
            _ => {
                changed.insert(key.clone());
                version_targets.insert(key);
            }
        }
    }
    snapshot.validate()?;
    let actor_subject = crate::AuthenticatedSubject {
        principal: authorization.principal().clone(),
        claims: Default::default(),
    };
    let changed_policy_ids = changes
        .operations
        .iter()
        .filter_map(|operation| match &operation.operation {
            ControlOperation::PutPathPolicy(policy) => Some(policy.id.as_str()),
            ControlOperation::DeletePathPolicy { id } => Some(id.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    for id in &changed_policy_ids {
        if let Some(policy) = snapshot
            .path_policies
            .iter()
            .find(|policy| policy.id == *id)
        {
            crate::control_admin_policy::validate_delegated_policy(
                &actor_subject,
                policy,
                &authority_snapshot,
            )
            .map_err(|error| {
                Error::ControlValidation(format!(
                    "path policy delegation exceeds actor authority: {error:?}"
                ))
            })?;
        }
        if let Some(previous_deny) = authority_snapshot
            .path_policies
            .iter()
            .find(|policy| policy.id == *id && policy.effect == PolicyEffect::Deny)
        {
            let replacement = snapshot
                .path_policies
                .iter()
                .find(|policy| policy.id == *id);
            if !replacement.is_some_and(|candidate| deny_preserves(candidate, previous_deny)) {
                let mut removed_capability = previous_deny.clone();
                removed_capability.effect = PolicyEffect::Allow;
                crate::control_admin_policy::validate_delegated_policy(
                    &actor_subject,
                    &removed_capability,
                    &authority_snapshot,
                )
                .map_err(|error| {
                    Error::ControlValidation(format!(
                        "path policy deny removal exceeds actor authority: {error:?}"
                    ))
                })?;
            }
        }
    }
    for binding in changes.operations.iter().filter_map(|operation| {
        if let ControlOperation::PutRoleBinding(binding) = &operation.operation {
            Some(binding)
        } else {
            None
        }
    }) {
        crate::control_admin_policy::validate_role_binding_delegation(
            authorization.principal(),
            binding,
            &authority_snapshot,
            &snapshot,
        )
        .map_err(|error| {
            Error::ControlValidation(format!(
                "role binding delegation exceeds actor authority: {error:?}"
            ))
        })?;
    }
    for removed in changes.operations.iter().filter_map(|operation| {
        let ControlOperation::DeleteRoleBinding {
            principal,
            scope,
            role,
        } = &operation.operation
        else {
            return None;
        };
        authority_snapshot.role_bindings.iter().find(|binding| {
            binding.principal == *principal && binding.scope == *scope && binding.role == *role
        })
    }) {
        crate::control_admin_policy::validate_role_binding_removal(
            authorization.principal(),
            removed,
            &authority_snapshot,
        )
        .map_err(|error| {
            Error::ControlValidation(format!(
                "role binding deny removal exceeds actor authority: {error:?}"
            ))
        })?;
    }
    for catalog_id in touched_catalogs {
        let previous_catalog = catalog_resource_key(&authority_snapshot, &catalog_id);
        let current_catalog = catalog_resource_key(&snapshot, &catalog_id);
        let moved = previous_catalog.is_some()
            && current_catalog.is_some()
            && previous_catalog != current_catalog;
        record_resource_transition(
            previous_catalog,
            current_catalog,
            &mut changed,
            &mut version_targets,
            &mut obsolete_versions,
        );
        if !moved {
            continue;
        }
        let descendant_ids = authority_snapshot
            .config
            .collections
            .iter()
            .chain(snapshot.config.collections.iter())
            .filter(|collection| collection.catalog == catalog_id)
            .map(|collection| collection.id.clone())
            .collect::<BTreeSet<_>>();
        for collection_id in descendant_ids {
            record_resource_transition(
                collection_resource_key(&authority_snapshot, &collection_id),
                collection_resource_key(&snapshot, &collection_id),
                &mut changed,
                &mut version_targets,
                &mut obsolete_versions,
            );
        }
    }
    for collection_id in touched_collections {
        record_resource_transition(
            collection_resource_key(&authority_snapshot, &collection_id),
            collection_resource_key(&snapshot, &collection_id),
            &mut changed,
            &mut version_targets,
            &mut obsolete_versions,
        );
    }
    let changed_resources: Vec<String> = changed.into_iter().collect();
    for resource in obsolete_versions {
        entity_versions.remove(&resource);
    }
    for resource in version_targets {
        entity_versions.insert(resource, revision.to_string());
    }
    Ok(AppliedControlChangeSet {
        snapshot,
        entity_versions,
        changed_resources,
    })
}

fn operation_expected_key(
    operation: &ControlOperation,
    authority_snapshot: &ControlSnapshot,
    candidate_snapshot: &ControlSnapshot,
) -> Result<String> {
    match operation {
        ControlOperation::PutCatalog(catalog) => {
            catalog_resource_key(authority_snapshot, &catalog.id)
                .map(Ok)
                .unwrap_or_else(|| operation_key(operation, candidate_snapshot))
        }
        ControlOperation::PutCollection(collection) => {
            collection_resource_key(authority_snapshot, &collection.id)
                .map(Ok)
                .unwrap_or_else(|| operation_key(operation, candidate_snapshot))
        }
        _ => operation_key(operation, candidate_snapshot),
    }
}

fn catalog_resource_key(snapshot: &ControlSnapshot, catalog_id: &str) -> Option<String> {
    snapshot
        .config
        .catalogs
        .iter()
        .find(|catalog| catalog.id == catalog_id)
        .map(|catalog| format!("tenant/{}/catalog/{}", catalog.tenant, catalog.id))
}

fn collection_resource_key(snapshot: &ControlSnapshot, collection_id: &str) -> Option<String> {
    let collection = snapshot
        .config
        .collections
        .iter()
        .find(|collection| collection.id == collection_id)?;
    let catalog = snapshot
        .config
        .catalogs
        .iter()
        .find(|catalog| catalog.id == collection.catalog)?;
    Some(format!(
        "tenant/{}/catalog/{}/collection/{}",
        catalog.tenant, catalog.id, collection.id
    ))
}

fn migrate_collection_control_state(
    snapshot: &mut ControlSnapshot,
    candidate: &CollectionDecl,
) -> Result<()> {
    let Some(current) = snapshot
        .config
        .collections
        .iter()
        .find(|collection| collection.id == candidate.id)
    else {
        return Ok(());
    };
    if current.catalog == candidate.catalog {
        return Ok(());
    }

    let old_scope = collection_scope(&snapshot.config, current)?;
    let new_scope = collection_scope(&snapshot.config, candidate)?;
    let old_anchor = collection_control_anchor(&snapshot.config, current)?;
    let new_anchor = collection_control_anchor(&snapshot.config, candidate)?;
    let policy_rewrites = snapshot
        .path_policies
        .iter()
        .enumerate()
        .filter(|(_, policy)| policy.scope.as_ref() == Some(&old_scope))
        .map(|(index, policy)| {
            policy
                .patterns
                .iter()
                .map(|pattern| rewrite_collection_policy_pattern(pattern, &old_anchor, &new_anchor))
                .collect::<Result<Vec<_>>>()
                .map(|patterns| (index, patterns))
        })
        .collect::<Result<Vec<_>>>()?;

    for binding in &mut snapshot.role_bindings {
        if binding.scope == old_scope {
            binding.scope = new_scope.clone();
        }
    }
    let mut unique_bindings = HashSet::new();
    snapshot
        .role_bindings
        .retain(|binding| unique_bindings.insert(binding.clone()));
    for (index, patterns) in policy_rewrites {
        snapshot.path_policies[index].scope = Some(new_scope.clone());
        snapshot.path_policies[index].patterns = patterns;
    }
    for tombstone in &mut snapshot.tombstoned_resources {
        if *tombstone == old_scope {
            *tombstone = new_scope.clone();
        }
    }
    let mut unique_tombstones = HashSet::new();
    snapshot
        .tombstoned_resources
        .retain(|scope| unique_tombstones.insert(scope.clone()));
    Ok(())
}

fn collection_scope(config: &AppConfig, collection: &CollectionDecl) -> Result<ControlScope> {
    let catalog = config
        .catalogs
        .iter()
        .find(|catalog| catalog.id == collection.catalog)
        .ok_or_else(|| {
            Error::ControlValidation(format!(
                "collection '{}' references unknown catalog '{}'",
                collection.id, collection.catalog
            ))
        })?;
    Ok(ControlScope::Collection {
        tenant_id: catalog.tenant.clone(),
        catalog_id: catalog.id.clone(),
        collection_id: collection.id.clone(),
    })
}

fn collection_control_anchor(config: &AppConfig, collection: &CollectionDecl) -> Result<String> {
    let catalog = config
        .catalogs
        .iter()
        .find(|catalog| catalog.id == collection.catalog)
        .ok_or_else(|| {
            Error::ControlValidation(format!(
                "collection '{}' references unknown catalog '{}'",
                collection.id, collection.catalog
            ))
        })?;
    let tenant = config
        .tenants
        .iter()
        .find(|tenant| tenant.id == catalog.tenant)
        .ok_or_else(|| {
            Error::ControlValidation(format!(
                "catalog '{}' references unknown tenant '{}'",
                catalog.id, catalog.tenant
            ))
        })?;
    Ok(format!(
        "/_control/v1/tenants/{}/catalogs/{}/collections/{}",
        tenant.external_id(),
        catalog.external_id(),
        collection.external_id()
    ))
}

fn rewrite_collection_policy_pattern(
    pattern: &str,
    old_anchor: &str,
    new_anchor: &str,
) -> Result<String> {
    if old_anchor == new_anchor {
        return Ok(pattern.to_string());
    }
    if let Some(suffix) = pattern.strip_prefix(old_anchor) {
        if suffix.is_empty() || suffix.starts_with('/') {
            return Ok(format!("{new_anchor}{suffix}"));
        }
    }

    let pattern_compiled = crate::control_admin_path::CompiledPathPattern::compile(pattern)
        .map_err(|_| {
            Error::ControlValidation(format!(
                "collection move cannot preserve policy pattern '{pattern}'"
            ))
        })?;
    let old_envelope =
        crate::control_admin_path::CompiledPathPattern::compile(&format!("{old_anchor}/**"))
            .map_err(|_| {
                Error::ControlValidation("invalid old collection policy envelope".to_string())
            })?;
    let new_envelope =
        crate::control_admin_path::CompiledPathPattern::compile(&format!("{new_anchor}/**"))
            .map_err(|_| {
                Error::ControlValidation("invalid new collection policy envelope".to_string())
            })?;
    if old_envelope.covers(&pattern_compiled) && new_envelope.covers(&pattern_compiled) {
        return Ok(pattern.to_string());
    }
    Err(Error::ControlValidation(format!(
        "collection move cannot preserve policy pattern '{pattern}'"
    )))
}

fn record_resource_transition(
    previous: Option<String>,
    current: Option<String>,
    changed: &mut BTreeSet<String>,
    version_targets: &mut BTreeSet<String>,
    obsolete_versions: &mut BTreeSet<String>,
) {
    if let Some(previous) = previous {
        changed.insert(previous.clone());
        if current.as_ref() != Some(&previous) {
            obsolete_versions.insert(previous);
        }
    }
    if let Some(current) = current {
        changed.insert(current.clone());
        version_targets.insert(current);
    }
}

fn operation_scopes(
    operation: &ControlOperation,
    authority_snapshot: &ControlSnapshot,
    candidate_snapshot: &ControlSnapshot,
) -> Result<Vec<ControlScope>> {
    match operation {
        ControlOperation::ReplacePlatformSettings(_) => Ok(vec![ControlScope::Platform]),
        ControlOperation::PutTenant(tenant) => Ok(vec![authority_snapshot
            .config
            .tenants
            .iter()
            .find(|existing| existing.id == tenant.id)
            .map_or(ControlScope::Platform, |existing| ControlScope::Tenant {
                tenant_id: existing.id.clone(),
            })]),
        ControlOperation::PutCatalog(catalog) => {
            let candidate_parent = ControlScope::Tenant {
                tenant_id: catalog.tenant.clone(),
            };
            let Some(existing) = authority_snapshot
                .config
                .catalogs
                .iter()
                .find(|existing| existing.id == catalog.id)
            else {
                return Ok(vec![candidate_parent]);
            };
            let existing_catalog = ControlScope::Catalog {
                tenant_id: existing.tenant.clone(),
                catalog_id: existing.id.clone(),
            };
            if existing.tenant == catalog.tenant {
                Ok(vec![existing_catalog])
            } else {
                Ok(vec![existing_catalog, candidate_parent])
            }
        }
        ControlOperation::PutCollection(collection) => {
            let candidate_catalog = candidate_snapshot
                .config
                .catalogs
                .iter()
                .find(|catalog| catalog.id == collection.catalog)
                .ok_or_else(|| {
                    Error::ControlValidation(format!(
                        "collection '{}' references unknown catalog '{}'",
                        collection.id, collection.catalog
                    ))
                })?;
            let candidate_parent = ControlScope::Catalog {
                tenant_id: candidate_catalog.tenant.clone(),
                catalog_id: candidate_catalog.id.clone(),
            };
            let Some(existing) = authority_snapshot
                .config
                .collections
                .iter()
                .find(|existing| existing.id == collection.id)
            else {
                return Ok(vec![candidate_parent]);
            };
            let existing_catalog = authority_snapshot
                .config
                .catalogs
                .iter()
                .find(|catalog| catalog.id == existing.catalog)
                .ok_or_else(|| {
                    Error::ControlValidation(format!(
                        "collection '{}' references unknown catalog '{}'",
                        existing.id, existing.catalog
                    ))
                })?;
            let existing_collection = ControlScope::Collection {
                tenant_id: existing_catalog.tenant.clone(),
                catalog_id: existing_catalog.id.clone(),
                collection_id: existing.id.clone(),
            };
            if existing.catalog == collection.catalog {
                Ok(vec![existing_collection])
            } else {
                Ok(vec![existing_collection, candidate_parent])
            }
        }
        ControlOperation::TombstoneResource { scope }
        | ControlOperation::PermanentlyDeleteResource { scope }
        | ControlOperation::DeleteRoleBinding { scope, .. } => Ok(vec![scope.clone()]),
        ControlOperation::PutRoleBinding(binding) => Ok(vec![binding.scope.clone()]),
        ControlOperation::PutPathPolicy(policy) => {
            let candidate_scope = policy.scope.clone().ok_or_else(|| {
                Error::ControlValidation(
                    "new path policy mutations require an explicit scope".to_string(),
                )
            })?;
            let Some(existing) = authority_snapshot
                .path_policies
                .iter()
                .find(|existing| existing.id == policy.id)
            else {
                return Ok(vec![candidate_scope]);
            };
            let existing_scope = existing.scope.clone().unwrap_or(ControlScope::Platform);
            if existing_scope == candidate_scope {
                Ok(vec![candidate_scope])
            } else {
                Ok(vec![existing_scope, candidate_scope])
            }
        }
        ControlOperation::DeletePathPolicy { id } => authority_snapshot
            .path_policies
            .iter()
            .find(|policy| policy.id == *id)
            .and_then(|policy| policy.scope.clone())
            .map(|scope| vec![scope])
            .ok_or_else(|| {
                Error::ControlValidation(format!(
                    "path policy deletion references unknown or legacy policy '{id}'"
                ))
            }),
    }
}

fn deny_preserves(candidate: &PathPolicy, previous: &PathPolicy) -> bool {
    if candidate.effect != PolicyEffect::Deny
        || candidate.role != previous.role
        || !previous
            .methods
            .iter()
            .all(|method| candidate.methods.contains(method))
    {
        return false;
    }
    let (Some(candidate_scope), Some(previous_scope)) = (&candidate.scope, &previous.scope) else {
        return false;
    };
    if !candidate_scope.contains(previous_scope) {
        return false;
    }
    let Ok(candidate_patterns) = candidate.validate() else {
        return false;
    };
    let Ok(previous_patterns) = previous.validate() else {
        return false;
    };
    previous_patterns.iter().all(|previous| {
        candidate_patterns
            .iter()
            .any(|candidate| candidate.covers(previous))
    })
}

fn operation_key(operation: &ControlOperation, snapshot: &ControlSnapshot) -> Result<String> {
    match operation {
        ControlOperation::ReplacePlatformSettings(_) => Ok("platform".to_string()),
        ControlOperation::PutTenant(tenant) => Ok(format!("tenant/{}", tenant.id)),
        ControlOperation::PutCatalog(catalog) => {
            Ok(format!("tenant/{}/catalog/{}", catalog.tenant, catalog.id))
        }
        ControlOperation::PutCollection(collection) => {
            let catalog = snapshot
                .config
                .catalogs
                .iter()
                .find(|catalog| catalog.id == collection.catalog)
                .ok_or_else(|| {
                    Error::ControlValidation(format!(
                        "collection '{}' references unknown catalog '{}'",
                        collection.id, collection.catalog
                    ))
                })?;
            Ok(format!(
                "tenant/{}/catalog/{}/collection/{}",
                catalog.tenant, catalog.id, collection.id
            ))
        }
        ControlOperation::TombstoneResource { scope }
        | ControlOperation::PermanentlyDeleteResource { scope } => Ok(scope.resource_key()),
        ControlOperation::PutRoleBinding(binding) => Ok(format!(
            "role-binding/{}/{}/{}/{}",
            binding.scope.resource_key(),
            binding.role,
            binding.principal.issuer,
            binding.principal.subject
        )),
        ControlOperation::DeleteRoleBinding {
            principal,
            scope,
            role,
        } => Ok(format!(
            "role-binding/{}/{}/{}/{}",
            scope.resource_key(),
            role,
            principal.issuer,
            principal.subject
        )),
        ControlOperation::PutPathPolicy(policy) => Ok(format!("path-policy/{}", policy.id)),
        ControlOperation::DeletePathPolicy { id } => Ok(format!("path-policy/{id}")),
    }
}

fn apply_operation(snapshot: &mut ControlSnapshot, operation: &ControlOperation) -> Result<()> {
    match operation {
        ControlOperation::ReplacePlatformSettings(config) => snapshot.config = config.clone(),
        ControlOperation::PutTenant(tenant) => {
            upsert_by_id(&mut snapshot.config.tenants, tenant.clone(), |item| {
                &item.id
            })
        }
        ControlOperation::PutCatalog(catalog) => {
            upsert_by_id(&mut snapshot.config.catalogs, catalog.clone(), |item| {
                &item.id
            })
        }
        ControlOperation::PutCollection(collection) => upsert_by_id(
            &mut snapshot.config.collections,
            collection.clone(),
            |item| &item.id,
        ),
        ControlOperation::TombstoneResource { scope } => {
            scope.validate_against(&snapshot.config)?;
            if !snapshot.tombstoned_resources.contains(scope) {
                snapshot.tombstoned_resources.push(scope.clone());
            }
        }
        ControlOperation::PermanentlyDeleteResource { scope } => {
            permanently_delete(&mut snapshot.config, scope)?;
            snapshot
                .tombstoned_resources
                .retain(|tombstone| tombstone != scope);
        }
        ControlOperation::PutRoleBinding(binding) => {
            if !snapshot.role_bindings.contains(binding) {
                snapshot.role_bindings.push(binding.clone());
            }
        }
        ControlOperation::DeleteRoleBinding {
            principal,
            scope,
            role,
        } => {
            principal.validate("role binding deletion")?;
            scope.validate_against(&snapshot.config)?;
            if role.trim().is_empty() {
                return Err(Error::ControlValidation(
                    "role binding deletion role must not be empty".to_string(),
                ));
            }
            snapshot.role_bindings.retain(|binding| {
                binding.principal != *principal || binding.scope != *scope || binding.role != *role
            });
        }
        ControlOperation::PutPathPolicy(policy) => {
            upsert_by_id(&mut snapshot.path_policies, policy.clone(), |item| &item.id)
        }
        ControlOperation::DeletePathPolicy { id } => {
            if id.trim().is_empty() {
                return Err(Error::ControlValidation(
                    "path policy deletion id must not be empty".to_string(),
                ));
            }
            snapshot.path_policies.retain(|policy| policy.id != *id);
        }
    }
    Ok(())
}

fn upsert_by_id<T, F>(items: &mut Vec<T>, candidate: T, id: F)
where
    F: Fn(&T) -> &String,
{
    if let Some(index) = items.iter().position(|item| id(item) == id(&candidate)) {
        items[index] = candidate;
    } else {
        items.push(candidate);
    }
}

fn permanently_delete(config: &mut AppConfig, scope: &ControlScope) -> Result<()> {
    scope.validate_against(config)?;
    match scope {
        ControlScope::Platform => {
            return Err(Error::ControlValidation(
                "the platform scope cannot be permanently deleted".to_string(),
            ))
        }
        ControlScope::Tenant { tenant_id } => {
            config.tenants.retain(|tenant| tenant.id != *tenant_id)
        }
        ControlScope::Catalog { catalog_id, .. } => {
            config.catalogs.retain(|catalog| catalog.id != *catalog_id)
        }
        ControlScope::Collection { collection_id, .. } => config
            .collections
            .retain(|collection| collection.id != *collection_id),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::rewrite_collection_policy_pattern;

    #[test]
    fn collection_move_rewrites_anchored_patterns_preserves_safe_intersections_and_rejects_ambiguity(
    ) {
        let old = "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a";
        let new = "/_control/v1/tenants/tenant-a/catalogs/catalog-a2/collections/collection-a";
        assert_eq!(
            rewrite_collection_policy_pattern(&format!("{old}/assets/**"), old, new).unwrap(),
            format!("{new}/assets/**")
        );

        let parent_agnostic =
            "/_control/v1/tenants/tenant-a/catalogs/catalog-a/collections/collection-a/metadata";
        assert_eq!(
            rewrite_collection_policy_pattern(
                parent_agnostic,
                "/_control/v1/tenants/*/catalogs/catalog-a/collections/collection-a",
                "/_control/v1/tenants/tenant-a/catalogs/*/collections/collection-a",
            )
            .unwrap(),
            parent_agnostic
        );

        assert!(rewrite_collection_policy_pattern(&format!("{old}-shadow/**"), old, new,).is_err());
    }
}
