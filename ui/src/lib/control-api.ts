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

export interface ControlReadClient {
  session(): Promise<ControlSessionView>;
  overview(): Promise<ControlOverview>;
  tenants(after?: string): Promise<ControlPage<TenantView>>;
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

type UnknownRecord = Record<string, unknown>;
const MAX_CURSOR_LENGTH = 512;
const MAX_PROBLEM_FIELD_LENGTH = 256;
const MAX_PROBLEM_CODE_LENGTH = 128;
const MAX_U64 = '18446744073709551615';

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

  effectiveSettings(signal?: AbortSignal): Promise<EffectiveSettingsView> {
    return this.read('/_control/v1/platform/effective-settings', 'Effective settings', readEffectiveSettings, signal);
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
      payload = await response.json();
    } catch {
      throw new ControlApiError(response.status, `${label} is unavailable.`);
    }
    const result = parse(payload);
    if (!result) throw new ControlApiError(response.status, `${label} is unavailable.`);
    return result;
  }
}
