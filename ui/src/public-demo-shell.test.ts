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

  it('gives public-demo visitors safe next steps without overstating the demo', () => {
    mountPublicDemoShell(document.body);

    const onwardLinks = Array.from(
      document.querySelectorAll<HTMLAnchorElement>('.public-demo-shell__resources a'),
    );
    expect(onwardLinks.map((link) => link.href)).toEqual([
      'https://github.com/ccancellieri/tellurion#quickstart',
      'https://ccancellieri.github.io/tellurion-demos/',
      'https://github.com/ccancellieri/tellurion/issues/new?template=evaluation.yml',
    ]);
    expect(onwardLinks.map((link) => link.textContent?.trim())).toEqual([
      'Build from source',
      'Verified demos',
      'Share evaluation feedback',
    ]);
    expect(onwardLinks.every((link) => link.target === '_blank')).toBe(true);
    expect(onwardLinks.every((link) => link.rel === 'noopener noreferrer')).toBe(true);
    expect(
      document
        .querySelector('main')
        ?.lastElementChild?.classList.contains('public-demo-shell__resources'),
    ).toBe(true);
    expect(document.body.textContent).toContain('Public HTTPS resources only.');
    expect(document.body.textContent).toContain('Temporary layers expire within 15 minutes.');
    expect(document.body.textContent).toContain('No account, upload, persistent tenant, or catalog is created.');
  });
});
