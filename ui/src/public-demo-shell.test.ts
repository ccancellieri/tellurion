/** @vitest-environment happy-dom */
import { afterEach, describe, expect, it } from 'vitest';
import { mountPublicDemoShell } from './public-demo-shell';

afterEach(() => {
  document.body.replaceChildren();
});

describe('public demo shell', () => {
  it('mounts only the direct-source workflow and its dedicated temporary map', () => {
    mountPublicDemoShell(document.body);

    expect(document.querySelector('tellurion-demo-source-workflow')).not.toBeNull();
    expect(document.querySelector('tellurion-demo-map-viewer')).not.toBeNull();
    expect(document.querySelector('tellurion-status-widget')).toBeNull();
    expect(document.querySelector('tellurion-operator-workspace')).toBeNull();
    expect(document.querySelector('.protocol-lab')).toBeNull();
    expect(
      document.querySelector<HTMLAnchorElement>('a[href="./THIRD_PARTY_NOTICES.txt"]')
        ?.textContent,
    ).toBe('Third-party notices');
  });
});
