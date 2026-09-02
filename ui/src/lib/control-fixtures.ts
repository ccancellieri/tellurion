import type {
  ControlAuditPage,
  ControlOverview,
  ControlPage,
  ControlReadClient,
  ControlSessionView,
  EffectiveSettingsView,
  TenantView,
} from './control-api';

function immutable<T>(value: T): T {
  if (value && typeof value === 'object') {
    Object.freeze(value);
    for (const child of Object.values(value as Record<string, unknown>)) immutable(child);
  }
  return value;
}

function copy<T>(value: T): T {
  return structuredClone(value);
}

export const controlFixtures = immutable({
  revision: 12,
  session: {
    authenticated: true,
    principal: 'fixture-operator',
    csrfToken: 'fixture-csrf',
    expiresAt: '2030-01-01T00:00:00.000Z',
  } satisfies ControlSessionView,
  platform: {
    scope: 'self',
    storeRevision: 12,
    appliedRevision: 12,
    lag: 0,
    lastSuccessfulRefreshUnixMs: 1_893_456_000_000,
    pollFailures: 0,
    activationFailures: 0,
    configVersion: 'fixture-revision-12',
  } satisfies ControlOverview,
  tenants: [{
    controlRevision: 12,
    entityVersion: '12',
    resource: { id: 'fixture-tenant', settings: {}, tombstoned: false },
  }] satisfies TenantView[],
  settings: {
    appliedRevision: 12,
    effective: {
      node: { level: 'platform' },
      settings: {
        cache_ttl_s: { value: 300, provenance: { kind: 'built_in_default' } },
      },
    },
  } satisfies EffectiveSettingsView,
  audit: [{
    revision: 12,
    actor: { issuer: 'https://fixture-issuer.test', subject: 'fixture-operator' },
    method: 'PUT',
    canonicalPath: '/_control/v1/platform/settings',
    correlationId: 'fixture-correlation',
    changedResources: ['platform'],
    recordedAtUnixMs: 1_893_456_000_000,
    applyingInstance: 'fixture-instance',
  }] satisfies ControlAuditPage['items'],
});

export const fixturePlatformSetting = immutable({
  key: 'cache_ttl_s',
  initialValue: 300,
});

export interface FixtureSettingSimulation {
  before: number;
  after: number;
  changed: boolean;
  impacts: readonly string[];
}

export function simulateFixturePlatformSetting(draft: string): FixtureSettingSimulation {
  const after = Number(draft);
  if (!Number.isSafeInteger(after) || after < 1 || after > 86_400) {
    throw new RangeError('Enter a whole cache lifetime from 1 to 86400 seconds.');
  }
  const before = fixturePlatformSetting.initialValue;
  const changed = before !== after;
  return immutable({
    before,
    after,
    changed,
    impacts: changed
      ? [
          `Cache entries created after a future apply would use a ${after}-second lifetime.`,
          'Existing cache entries and the running service remain unchanged in this demonstration.',
        ]
      : ['The proposed value matches the fixture baseline; no future cache behavior would differ.'],
  });
}

export class FixtureControlReadClient implements ControlReadClient {
  async session(): Promise<ControlSessionView> {
    return copy(controlFixtures.session);
  }

  async overview(): Promise<ControlOverview> {
    return copy(controlFixtures.platform);
  }

  async tenants(after?: string): Promise<ControlPage<TenantView>> {
    const items = after === undefined ? controlFixtures.tenants : [];
    return copy({
      controlRevision: controlFixtures.revision,
      items,
      ...(after === undefined ? { nextAfter: controlFixtures.tenants.at(-1)?.resource.id } : {}),
    });
  }

  async effectiveSettings(): Promise<EffectiveSettingsView> {
    return copy(controlFixtures.settings);
  }

  async audit(after?: string): Promise<ControlAuditPage> {
    const items = after === undefined ? controlFixtures.audit : [];
    return copy({
      revision: controlFixtures.revision,
      items,
      ...(after === undefined ? { nextAfter: String(controlFixtures.audit.at(-1)?.revision) } : {}),
    });
  }
}
