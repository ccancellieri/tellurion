import {
  ControlApiError,
  ControlEntityConflictError,
  ControlSignInRequiredError,
  ControlUncertainWriteError,
  type PlatformSettingsCommit,
  type PlatformSettingsEnvelope,
  type PlatformSettingsPreview,
  ProductionControlReadClient,
} from './control-api';

export type PlatformSettingsEditPhase =
  'unloaded' | 'loading' | 'editing' | 'previewing' | 'previewed' | 'applying' | 'uncertain' | 'applied';

export interface PlatformSettingsEditView {
  phase: PlatformSettingsEditPhase;
  controlRevision?: number;
  entityVersion?: string;
  draft?: Record<string, unknown>;
  preview?: PlatformSettingsPreview;
  commit?: PlatformSettingsCommit;
  needsRebase: boolean;
}

interface FrozenSettingsRequest {
  body: string;
  csrfToken: string;
}

export class PlatformSettingsEditSession {
  private readonly client: ProductionControlReadClient;
  private readonly newIdempotencyKey: () => string;
  #phase: PlatformSettingsEditPhase = 'unloaded';
  #source?: PlatformSettingsEnvelope;
  #draft?: Record<string, unknown>;
  #principal?: string;
  #csrfToken?: string;
  #frozen?: FrozenSettingsRequest;
  #preview?: PlatformSettingsPreview;
  #commit?: PlatformSettingsCommit;
  #needsRebase = false;

  constructor(
    client: ProductionControlReadClient,
    newIdempotencyKey: () => string = () => crypto.randomUUID(),
  ) {
    this.client = client;
    this.newIdempotencyKey = newIdempotencyKey;
  }

  view(): PlatformSettingsEditView {
    return {
      phase: this.#phase,
      needsRebase: this.#needsRebase,
      ...(this.#source ? {
        controlRevision: this.#source.controlRevision,
        entityVersion: this.#source.entityVersion,
      } : {}),
      ...(this.#draft ? { draft: structuredClone(this.#draft) } : {}),
      ...(this.#preview ? { preview: structuredClone(this.#preview) } : {}),
      ...(this.#commit ? { commit: { ...this.#commit, changedResources: [...this.#commit.changedResources] } } : {}),
    };
  }

  async load(): Promise<void> {
    if (this.#phase !== 'unloaded' && this.#phase !== 'applied') {
      throw new ControlApiError(0, 'Finish the current settings draft before loading another one.');
    }
    const previousPhase = this.#phase;
    this.#phase = 'loading';
    try {
      const session = await this.client.session();
      if (!session.authenticated) throw new ControlSignInRequiredError();
      const source = await this.client.platformSettings();
      this.#source = source;
      this.#draft = structuredClone(source.resource);
      this.#principal = session.principal;
      this.#csrfToken = session.csrfToken;
      this.#frozen = undefined;
      this.#preview = undefined;
      this.#commit = undefined;
      this.#needsRebase = false;
      this.#phase = 'editing';
    } catch (error) {
      this.#phase = previousPhase;
      throw error;
    }
  }

  async rebaseAfterConflict(): Promise<void> {
    if (this.#phase !== 'editing' || !this.#needsRebase || !this.#draft) {
      throw new ControlApiError(0, 'Only a conflicted settings draft can be rebased.');
    }
    this.#phase = 'loading';
    try {
      const session = await this.client.session();
      if (!session.authenticated || session.principal !== this.#principal) throw new ControlSignInRequiredError();
      const source = await this.client.platformSettings();
      const cacheTtl = this.#draft.cache_ttl_s;
      this.#source = source;
      this.#draft = { ...structuredClone(source.resource), cache_ttl_s: cacheTtl };
      this.#csrfToken = session.csrfToken;
      this.#needsRebase = false;
      this.#phase = 'editing';
    } catch (error) {
      this.#phase = 'editing';
      throw error;
    }
  }

  editCacheTtlSeconds(value: number | null): void {
    if (this.#phase !== 'editing' && this.#phase !== 'previewed') {
      throw new ControlApiError(0, 'Settings cannot be edited in the current state.');
    }
    if (value !== null && (!Number.isSafeInteger(value) || value < 0)) {
      throw new ControlApiError(400, 'Enter a non-negative whole cache lifetime.');
    }
    this.#draft = { ...this.#draft, cache_ttl_s: value };
    this.#frozen = undefined;
    this.#preview = undefined;
    this.#phase = 'editing';
  }

  async preview(): Promise<PlatformSettingsPreview> {
    if (this.#phase !== 'editing' || !this.#source || !this.#draft) {
      throw new ControlApiError(0, 'A settings draft is required before preview.');
    }
    if (!this.#csrfToken) {
      throw new ControlApiError(401, 'An authenticated control session with CSRF is required.');
    }
    if (this.#needsRebase) {
      throw new ControlApiError(409, 'Refresh the conflicted settings draft before preview.');
    }
    const idempotencyKey = this.newIdempotencyKey();
    if (!idempotencyKey.trim()) throw new ControlApiError(400, 'A valid settings request key is required.');
    const frozen = {
      body: JSON.stringify({
        idempotency_key: idempotencyKey,
        operations: [{
          expected_entity_version: this.#source.entityVersion,
          operation: { SetPlatformSettings: structuredClone(this.#draft) },
        }],
      }),
      csrfToken: this.#csrfToken,
    };
    this.#phase = 'previewing';
    try {
      const result = await this.client.previewPlatformSettings(frozen.body, frozen.csrfToken);
      if (result.changedResources.length !== 1 || result.changedResources[0] !== 'platform' ||
        !result.entityVersions.platform) {
        throw new ControlApiError(200, 'Platform settings preview is unavailable.');
      }
      this.#frozen = frozen;
      this.#preview = result;
      this.#phase = 'previewed';
      return structuredClone(result);
    } catch (error) {
      this.#frozen = undefined;
      this.#preview = undefined;
      if (error instanceof ControlEntityConflictError) this.#needsRebase = true;
      this.#phase = 'editing';
      throw error;
    }
  }

  async apply(): Promise<PlatformSettingsCommit> {
    if (this.#phase !== 'previewed' || !this.#frozen) {
      throw new ControlApiError(0, 'Preview the current settings draft before applying it.');
    }
    return this.sendApply(this.#frozen, false);
  }

  async retryUncertainApply(): Promise<PlatformSettingsCommit> {
    if (this.#phase !== 'uncertain' || !this.#frozen) {
      throw new ControlApiError(0, 'There is no uncertain settings apply to retry.');
    }
    return this.sendApply(this.#frozen, true);
  }

  private async sendApply(frozen: FrozenSettingsRequest, retry: boolean): Promise<PlatformSettingsCommit> {
    this.#phase = 'applying';
    try {
      const commit = await this.client.applyPlatformSettings(frozen.body, frozen.csrfToken);
      if (commit.changedResources.length !== 1 || commit.changedResources[0] !== 'platform') {
        throw new ControlUncertainWriteError();
      }
      this.#commit = commit;
      this.#frozen = undefined;
      this.#preview = undefined;
      this.#phase = 'applied';
      return { ...commit, changedResources: [...commit.changedResources] };
    } catch (error) {
      if (retry || error instanceof ControlUncertainWriteError) {
        this.#phase = 'uncertain';
      } else {
        this.#phase = 'editing';
        this.#frozen = undefined;
        this.#preview = undefined;
        if (error instanceof ControlEntityConflictError) this.#needsRebase = true;
      }
      throw error;
    }
  }
}
