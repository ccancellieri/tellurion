export interface ControlSessionView {
  authenticated: boolean;
  principal?: string;
  csrfToken?: string;
  expiresAt?: string;
}

export interface ControlOverview {
  scope: string;
  storeRevision: number;
  appliedRevision: number;
  lag: number;
  lastSuccessfulRefreshUnixMs?: number;
  pollFailures: number;
  activationFailures: number;
  configVersion: string;
}

export interface TenantView {
  controlRevision: number;
  entityVersion: string;
  resource: {
    id: string;
    settings: Record<string, unknown>;
    tombstoned: boolean;
  };
}

export interface ControlVisibility {
  public: boolean;
  sharedWith: string[];
}

export interface CatalogView {
  controlRevision: number;
  entityVersion: string;
  resource: {
    id: string;
    tenant: string;
    settings: Record<string, unknown>;
    visibility: ControlVisibility;
    tombstoned: boolean;
  };
}

export interface CollectionView {
  controlRevision: number;
  entityVersion: string;
  resource: {
    id: string;
    catalog: string;
    kind: 'vector' | 'raster' | 'record';
    settings: Record<string, unknown>;
    visibility: ControlVisibility;
    tombstoned: boolean;
  };
}

export interface ControlPage<T> {
  controlRevision: number;
  items: T[];
  nextAfter?: string;
}

export type ControlSettingsLevel = 'platform' | 'tenant' | 'catalog' | 'collection';

export type ControlSettingProvenance =
  | { kind: 'built_in_default' }
  | { kind: 'derived' }
  | { kind: 'local_override' }
  | { kind: 'inherited'; level: ControlSettingsLevel }
  | { kind: 'profile'; level: ControlSettingsLevel; profileId: string };

export interface ControlSettingValue {
  value: unknown;
  provenance: ControlSettingProvenance;
}

export interface EffectiveSettingsView {
  appliedRevision: number;
  effective: {
    node: {
      level: ControlSettingsLevel;
      tenant?: string;
      catalog?: string;
      collection?: string;
    };
    settings: Record<string, ControlSettingValue>;
  };
}

export interface ControlAuditItem {
  revision: number;
  actor: { issuer: string; subject: string };
  method: string;
  canonicalPath: string;
  correlationId: string;
  changedResources: string[];
  recordedAtUnixMs: number;
  applyingInstance: string;
}

export interface ControlAuditPage {
  revision: number;
  items: ControlAuditItem[];
  nextAfter?: string;
}

export interface PlatformSettingsEnvelope {
  controlRevision: number;
  entityVersion: string;
  resource: Record<string, unknown>;
}

export interface PlatformSettingsPreview {
  baseRevision: number;
  prospectiveRevision: number;
  changedResources: string[];
  entityVersions: Record<string, string>;
}

export interface PlatformSettingsCommit {
  revision: number;
  changedResources: string[];
  replayed: boolean;
}

export interface ControlReadClient {
  session(): Promise<ControlSessionView>;
  overview(): Promise<ControlOverview>;
  tenants(after?: string): Promise<ControlPage<TenantView>>;
  catalogs(tenant: string, after?: string): Promise<ControlPage<CatalogView>>;
  collections(tenant: string, catalog: string, after?: string): Promise<ControlPage<CollectionView>>;
  effectiveSettings(): Promise<EffectiveSettingsView>;
  audit(after?: string): Promise<ControlAuditPage>;
}

export class ControlApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
    this.name = 'ControlApiError';
  }
}

export class ControlSignInRequiredError extends ControlApiError {
  constructor() {
    super(401, 'Sign in is required to access the control service.');
    this.name = 'ControlSignInRequiredError';
  }
}

export class ControlForbiddenError extends ControlApiError {
  constructor() {
    super(403, 'You do not have access to this control resource.');
    this.name = 'ControlForbiddenError';
  }
}

export class ControlScopeAbsentError extends ControlApiError {
  constructor() {
    super(404, 'This control scope is no longer available.');
    this.name = 'ControlScopeAbsentError';
  }
}

export class ControlConflictError extends ControlApiError {
  constructor() {
    super(409, 'The control resource changed. Refresh and try again.');
    this.name = 'ControlConflictError';
  }
}

export class ControlEntityConflictError extends ControlApiError {
  constructor() {
    super(409, 'Platform settings changed. Refresh and preview the draft again.');
    this.name = 'ControlEntityConflictError';
  }
}

export class ControlUncertainWriteError extends ControlApiError {
  constructor() {
    super(0, 'The settings apply outcome is uncertain. Retry the same request before editing again.');
    this.name = 'ControlUncertainWriteError';
  }
}

type UnknownRecord = Record<string, unknown>;
const MAX_CURSOR_LENGTH = 512;
const MAX_PROBLEM_FIELD_LENGTH = 256;
const MAX_PROBLEM_CODE_LENGTH = 128;
const MAX_U64 = '18446744073709551615';

export function validControlScopeId(value: string): boolean {
  return value.length > 0 && value.length <= 128 && /^[A-Za-z0-9_-]+$/.test(value);
}

function record(value: unknown): UnknownRecord | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as UnknownRecord
    : null;
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

function unsignedSafeInteger(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : undefined;
}

function boundedText(value: unknown, limit: number): string | undefined {
  return typeof value === 'string' && value.length <= limit ? value : undefined;
}

function nonEmptyText(value: unknown): string | undefined {
  return typeof value === 'string' && value.length > 0 ? value : undefined;
}

function hasUnsafeIntegralJsonNumber(raw: string): boolean {
  let inString = false;
  let escaped = false;
  for (let index = 0; index < raw.length; index += 1) {
    const char = raw[index];
    if (inString) {
      if (escaped) escaped = false;
      else if (char === '\\') escaped = true;
      else if (char === '"') inString = false;
      continue;
    }
    if (char === '"') {
      inString = true;
      continue;
    }
    if (char !== '-' && (char < '0' || char > '9')) continue;
    let end = index + 1;
    while (end < raw.length && /[0-9eE+.-]/.test(raw[end])) end += 1;
    const token = raw.slice(index, end);
    if (/^-?(0|[1-9][0-9]*)$/.test(token) && !Number.isSafeInteger(Number(token))) return true;
    index = end - 1;
  }
  return false;
}

function nodeIdentifier(source: UnknownRecord, key: 'tenant' | 'catalog' | 'collection'): string | null | undefined {
  if (!Object.hasOwn(source, key)) return undefined;
  return nonEmptyText(source[key]) ?? null;
}

function settingLevel(value: unknown): ControlSettingsLevel | undefined {
  return value === 'platform' || value === 'tenant' || value === 'catalog' || value === 'collection'
    ? value
    : undefined;
}

function settingProvenance(value: unknown): ControlSettingProvenance | undefined {
  const source = record(value);
  if (!source || typeof source.kind !== 'string') return undefined;
  if (source.kind === 'built_in_default' || source.kind === 'derived' || source.kind === 'local_override') {
    return { kind: source.kind };
  }
  const level = settingLevel(source.level);
  if (!level) return undefined;
  if (source.kind === 'inherited') return { kind: 'inherited', level };
  if (source.kind === 'profile') {
    const profileId = nonEmptyText(source.profile_id);
    return profileId ? { kind: 'profile', level, profileId } : undefined;
  }
  return undefined;
}

function problemCode(value: unknown): string | undefined {
  const problem = record(value);
  if (!problem || !boundedText(problem.type, MAX_PROBLEM_FIELD_LENGTH) ||
    !boundedText(problem.title, MAX_PROBLEM_FIELD_LENGTH) ||
    unsignedSafeInteger(problem.status) === undefined) return undefined;
  return boundedText(problem.code, MAX_PROBLEM_CODE_LENGTH);
}

function readError(status: number, label: string, code?: string): ControlApiError {
  switch (status) {
    case 401: return new ControlSignInRequiredError();
    case 403: return new ControlForbiddenError();
    case 404: return new ControlScopeAbsentError();
    case 409: return code === 'ControlRevisionConflict'
      ? new ControlConflictError()
      : new ControlApiError(status, `${label} is unavailable.`);
    default: return new ControlApiError(status, `${label} is unavailable.`);
  }
}

function isAbortError(error: unknown): boolean {
  return record(error)?.name === 'AbortError';
}

async function readProblemCode(response: Response): Promise<string | undefined> {
  const contentType = response.headers.get('content-type') ?? '';
  if (!contentType.toLowerCase().includes('application/problem+json')) return undefined;
  try {
    return problemCode(await response.json());
  } catch {
    // Problem details are deliberately discarded so response content is never surfaced to operators.
    return undefined;
  }
}

function readSession(value: unknown): ControlSessionView | null {
  const source = record(value);
  if (!source || typeof source.authenticated !== 'boolean') return null;
  if (!source.authenticated) return { authenticated: false };
  const principal = text(source.principal);
  if (!principal) return null;
  const csrfToken = text(source.csrf_token);
  const expiresInSeconds = unsignedSafeInteger(source.expires_in_s);
  return {
    authenticated: true,
    principal,
    ...(csrfToken ? { csrfToken } : {}),
    ...(expiresInSeconds === undefined ? {} : { expiresAt: new Date(Date.now() + expiresInSeconds * 1_000).toISOString() }),
  };
}

function readOverview(value: unknown): ControlOverview | null {
  const source = record(value);
  if (!source) return null;
  const scope = text(source.scope);
  const storeRevision = unsignedSafeInteger(source.store_revision);
  const appliedRevision = unsignedSafeInteger(source.applied_revision);
  const lag = unsignedSafeInteger(source.lag);
  const pollFailures = unsignedSafeInteger(source.poll_failures);
  const activationFailures = unsignedSafeInteger(source.activation_failures);
  const configVersion = text(source.config_version);
  if (!scope || storeRevision === undefined || appliedRevision === undefined || lag === undefined ||
    pollFailures === undefined || activationFailures === undefined || !configVersion) return null;
  const refreshed = unsignedSafeInteger(source.last_successful_refresh_unix_ms);
  return {
    scope, storeRevision, appliedRevision, lag, pollFailures, activationFailures, configVersion,
    ...(refreshed === undefined ? {} : { lastSuccessfulRefreshUnixMs: refreshed }),
  };
}

function readTenants(value: unknown): ControlPage<TenantView> | null {
  const source = record(value);
  if (!source) return null;
  const controlRevision = unsignedSafeInteger(source.control_revision);
  if (controlRevision === undefined || !Array.isArray(source.items)) return null;
  const items: TenantView[] = [];
  for (const item of source.items) {
    const envelope = record(item);
    if (!envelope) return null;
    const resource = record(envelope.resource);
    if (!resource) return null;
    const itemRevision = unsignedSafeInteger(envelope.control_revision);
    const entityVersion = text(envelope.entity_version);
    const id = text(resource.id);
    const settings = record(resource.settings);
    if (itemRevision === undefined || !entityVersion || !id || !settings ||
      typeof resource.tombstoned !== 'boolean') return null;
    items.push({
      controlRevision: itemRevision,
      entityVersion,
      resource: { id, settings, tombstoned: resource.tombstoned },
    });
  }
  const nextAfter = text(source.next_after);
  return { controlRevision, items, ...(nextAfter ? { nextAfter } : {}) };
}

function readVisibility(value: unknown): ControlVisibility | null {
  const source = record(value);
  if (!source || typeof source.public !== 'boolean' || !Array.isArray(source.shared_with) ||
    !source.shared_with.every((entry) => typeof entry === 'string')) return null;
  return { public: source.public, sharedWith: source.shared_with };
}

function readScopedPage<T>(
  value: unknown,
  parseItem: (resource: UnknownRecord, controlRevision: number, entityVersion: string) => T | null,
): ControlPage<T> | null {
  const source = record(value);
  const controlRevision = source ? unsignedSafeInteger(source.control_revision) : undefined;
  if (controlRevision === undefined || !source || !Array.isArray(source.items)) return null;
  const items: T[] = [];
  for (const item of source.items) {
    const envelope = record(item);
    const resource = envelope ? record(envelope.resource) : null;
    const revision = envelope ? unsignedSafeInteger(envelope.control_revision) : undefined;
    const entityVersion = envelope ? nonEmptyText(envelope.entity_version) : undefined;
    if (!resource || revision === undefined || !entityVersion) return null;
    const parsed = parseItem(resource, revision, entityVersion);
    if (!parsed) return null;
    items.push(parsed);
  }
  const nextAfter = source.next_after;
  if (nextAfter !== undefined && nextAfter !== null && typeof nextAfter !== 'string') return null;
  return { controlRevision, items, ...(nextAfter ? { nextAfter } : {}) };
}

function readCatalogs(value: unknown, tenant: string): ControlPage<CatalogView> | null {
  return readScopedPage(value, (resource, controlRevision, entityVersion) => {
    const id = text(resource.id);
    const settings = record(resource.settings);
    const visibility = readVisibility(resource.visibility);
    if (!id || !validControlScopeId(id) || resource.tenant !== tenant || !settings || !visibility ||
      typeof resource.tombstoned !== 'boolean') return null;
    return { controlRevision, entityVersion, resource: {
      id, tenant, settings, visibility, tombstoned: resource.tombstoned,
    } };
  });
}

function readCollections(value: unknown, catalog: string): ControlPage<CollectionView> | null {
  return readScopedPage(value, (resource, controlRevision, entityVersion) => {
    const id = text(resource.id);
    const settings = record(resource.settings);
    const visibility = readVisibility(resource.visibility);
    const kind = resource.kind;
    if (!id || !validControlScopeId(id) || resource.catalog !== catalog || !settings || !visibility ||
      (kind !== 'vector' && kind !== 'raster' && kind !== 'record') ||
      typeof resource.tombstoned !== 'boolean') return null;
    return { controlRevision, entityVersion, resource: {
      id, catalog, kind, settings, visibility, tombstoned: resource.tombstoned,
    } };
  });
}

function readEffectiveSettings(value: unknown): EffectiveSettingsView | null {
  const source = record(value);
  if (!source) return null;
  const appliedRevision = unsignedSafeInteger(source.applied_revision);
  const effective = record(source.effective);
  if (!effective) return null;
  const node = record(effective.node);
  const settings = record(effective.settings);
  if (!node || !settings) return null;
  const level = settingLevel(node.level);
  if (appliedRevision === undefined || !level) return null;
  const tenant = nodeIdentifier(node, 'tenant');
  const catalog = nodeIdentifier(node, 'catalog');
  const collection = nodeIdentifier(node, 'collection');
  if (tenant === null || catalog === null || collection === null) return null;
  const validNode =
    (level === 'platform' && tenant === undefined && catalog === undefined && collection === undefined) ||
    (level === 'tenant' && tenant !== undefined && catalog === undefined && collection === undefined) ||
    (level === 'catalog' && tenant !== undefined && catalog !== undefined && collection === undefined) ||
    (level === 'collection' && tenant !== undefined && catalog !== undefined && collection !== undefined);
  if (!validNode) return null;
  const resolvedSettings: Record<string, ControlSettingValue> = {};
  for (const [key, entry] of Object.entries(settings)) {
    const pair = record(entry);
    if (!pair || !Object.hasOwn(pair, 'value')) return null;
    const provenance = settingProvenance(pair.provenance);
    if (!provenance) return null;
    resolvedSettings[key] = { value: pair.value, provenance };
  }
  return {
    appliedRevision,
    effective: {
      node: {
        level,
        ...(tenant ? { tenant } : {}),
        ...(catalog ? { catalog } : {}),
        ...(collection ? { collection } : {}),
      },
      settings: resolvedSettings,
    },
  };
}

function readAudit(value: unknown): ControlAuditPage | null {
  const source = record(value);
  if (!source) return null;
  const revision = unsignedSafeInteger(source.revision);
  if (revision === undefined || !Array.isArray(source.items)) return null;
  const items: ControlAuditItem[] = [];
  for (const item of source.items) {
    const raw = record(item);
    if (!raw) return null;
    const actor = record(raw.actor);
    if (!actor) return null;
    const itemRevision = unsignedSafeInteger(raw.revision);
    const issuer = text(actor.issuer);
    const subject = text(actor.subject);
    const method = text(raw.method);
    const canonicalPath = text(raw.canonical_path);
    const correlationId = text(raw.correlation_id);
    const recordedAtUnixMs = unsignedSafeInteger(raw.recorded_at_unix_ms);
    const applyingInstance = text(raw.applying_instance);
    if (itemRevision === undefined || !issuer || !subject || !method || !canonicalPath ||
      !correlationId || !Array.isArray(raw.changed_resources) || !raw.changed_resources.every((entry) => typeof entry === 'string') ||
      recordedAtUnixMs === undefined || !applyingInstance) return null;
    items.push({
      revision: itemRevision, actor: { issuer, subject }, method, canonicalPath, correlationId,
      changedResources: raw.changed_resources, recordedAtUnixMs, applyingInstance,
    });
  }
  const nextAfter = source.next_after;
  if (nextAfter !== undefined && nextAfter !== null && unsignedSafeInteger(nextAfter) === undefined) return null;
  return {
    revision,
    items,
    ...(nextAfter === undefined || nextAfter === null ? {} : { nextAfter: String(nextAfter) }),
  };
}

function readPlatformSettings(value: unknown): PlatformSettingsEnvelope | null {
  const source = record(value);
  if (!source) return null;
  const controlRevision = unsignedSafeInteger(source.control_revision);
  const entityVersion = nonEmptyText(source.entity_version);
  const resource = record(source.resource);
  return controlRevision === undefined || !entityVersion || !resource
    ? null
    : { controlRevision, entityVersion, resource };
}

function readPlatformPreview(value: unknown): PlatformSettingsPreview | null {
  const source = record(value);
  if (!source) return null;
  const baseRevision = unsignedSafeInteger(source.base_revision);
  const prospectiveRevision = unsignedSafeInteger(source.prospective_revision);
  const changedResources = source.changed_resources;
  const entityVersions = record(source.entity_versions);
  if (baseRevision === undefined || prospectiveRevision === undefined ||
    !Array.isArray(changedResources) || !changedResources.every((entry) => typeof entry === 'string') ||
    !entityVersions || !Object.values(entityVersions).every((entry) => typeof entry === 'string')) return null;
  return {
    baseRevision, prospectiveRevision, changedResources, entityVersions: entityVersions as Record<string, string>,
  };
}

function readPlatformCommit(value: unknown): PlatformSettingsCommit | null {
  const source = record(value);
  if (!source) return null;
  const revision = unsignedSafeInteger(source.revision);
  const changedResources = source.changed_resources;
  if (revision === undefined || !Array.isArray(changedResources) ||
    !changedResources.every((entry) => typeof entry === 'string') || typeof source.replayed !== 'boolean') return null;
  return { revision, changedResources, replayed: source.replayed };
}

export class ProductionControlReadClient implements ControlReadClient {
  async session(signal?: AbortSignal): Promise<ControlSessionView> {
    return this.read('/_auth/control/session', 'Control session', readSession, signal);
  }

  overview(signal?: AbortSignal): Promise<ControlOverview> {
    return this.read('/_control/v1/platform/overview', 'Platform overview', readOverview, signal);
  }

  async tenants(after?: string, signal?: AbortSignal): Promise<ControlPage<TenantView>> {
    return this.read(this.cursorPath('/_control/v1/tenants', after, 'Tenant list'), 'Tenant list', readTenants, signal);
  }

  async catalogs(tenant: string, after?: string, signal?: AbortSignal): Promise<ControlPage<CatalogView>> {
    if (!validControlScopeId(tenant)) throw new ControlApiError(400, 'Catalog list is unavailable.');
    const path = this.cursorPath(`/_control/v1/tenants/${tenant}/catalogs`, after, 'Catalog list');
    return this.read(path, 'Catalog list', (value) => readCatalogs(value, tenant), signal);
  }

  async collections(tenant: string, catalog: string, after?: string, signal?: AbortSignal): Promise<ControlPage<CollectionView>> {
    if (!validControlScopeId(tenant) || !validControlScopeId(catalog)) {
      throw new ControlApiError(400, 'Collection list is unavailable.');
    }
    const path = this.cursorPath(`/_control/v1/tenants/${tenant}/catalogs/${catalog}/collections`, after, 'Collection list');
    return this.read(path, 'Collection list', (value) => readCollections(value, catalog), signal);
  }

  effectiveSettings(signal?: AbortSignal): Promise<EffectiveSettingsView> {
    return this.read('/_control/v1/platform/effective-settings', 'Effective settings', readEffectiveSettings, signal);
  }

  platformSettings(signal?: AbortSignal): Promise<PlatformSettingsEnvelope> {
    return this.read('/_control/v1/platform/settings', 'Platform settings', readPlatformSettings, signal, true);
  }

  previewPlatformSettings(body: string, csrfToken: string): Promise<PlatformSettingsPreview> {
    return this.write('/_control/v1/platform/settings?dry_run=true', body, csrfToken, readPlatformPreview, false);
  }

  applyPlatformSettings(body: string, csrfToken: string): Promise<PlatformSettingsCommit> {
    return this.write('/_control/v1/platform/settings', body, csrfToken, readPlatformCommit, true);
  }

  async audit(after?: string, signal?: AbortSignal): Promise<ControlAuditPage> {
    if (after !== undefined && !this.isU64Cursor(after)) {
      throw new ControlApiError(400, 'Audit log is unavailable.');
    }
    return this.read(this.cursorPath('/_control/v1/platform/audit', after, 'Audit log'), 'Audit log', readAudit, signal);
  }

  private cursorPath(path: string, after: string | undefined, label: string): string {
    if (after !== undefined && after.length > MAX_CURSOR_LENGTH) {
      throw new ControlApiError(400, `${label} is unavailable.`);
    }
    return after === undefined ? path : `${path}?after=${encodeURIComponent(after)}`;
  }

  private isU64Cursor(value: string): boolean {
    return /^(0|[1-9][0-9]*)$/.test(value) && value.length <= MAX_U64.length &&
      (value.length < MAX_U64.length || value <= MAX_U64);
  }

  private async read<T>(
    path: string,
    label: string,
    parse: (value: unknown) => T | null,
    signal?: AbortSignal,
    preserveIntegralPrecision = false,
  ): Promise<T> {
    let response: Response;
    try {
      response = await fetch(path, {
        method: 'GET',
        credentials: 'include',
        headers: { Accept: 'application/json' },
        signal,
      });
    } catch (error) {
      if (isAbortError(error)) throw error;
      throw new ControlApiError(0, `${label} is unavailable.`);
    }
    if (!response.ok) {
      throw readError(response.status, label, await readProblemCode(response));
    }
    let payload: unknown;
    try {
      if (preserveIntegralPrecision) {
        const raw = await response.text();
        if (hasUnsafeIntegralJsonNumber(raw)) throw new Error('unsafe integer');
        payload = JSON.parse(raw);
      } else {
        payload = await response.json();
      }
    } catch {
      throw new ControlApiError(response.status, `${label} is unavailable.`);
    }
    const result = parse(payload);
    if (!result) throw new ControlApiError(response.status, `${label} is unavailable.`);
    return result;
  }

  private async write<T>(
    path: string,
    body: string,
    csrfToken: string,
    parse: (value: unknown) => T | null,
    mayHaveCommitted: boolean,
  ): Promise<T> {
    let response: Response;
    try {
      response = await fetch(path, {
        method: 'PUT',
        credentials: 'include',
        headers: {
          Accept: 'application/json',
          'Content-Type': 'application/json',
          'x-tellurion-csrf': csrfToken,
        },
        body,
      });
    } catch {
      throw mayHaveCommitted ? new ControlUncertainWriteError() : new ControlApiError(0, 'Platform settings preview is unavailable.');
    }
    if (!response.ok) {
      const code = await readProblemCode(response);
      if (response.status === 409 && code === 'ControlEntityVersionConflict') throw new ControlEntityConflictError();
      if (response.status === 409 && code === 'ControlRevisionConflict') throw new ControlConflictError();
      if (mayHaveCommitted && response.status >= 500) throw new ControlUncertainWriteError();
      throw readError(response.status, 'Platform settings', code);
    }
    let result: T | null;
    try {
      result = parse(await response.json());
    } catch {
      result = null;
    }
    if (!result) {
      throw mayHaveCommitted ? new ControlUncertainWriteError() : new ControlApiError(response.status, 'Platform settings preview is unavailable.');
    }
    return result;
  }
}
