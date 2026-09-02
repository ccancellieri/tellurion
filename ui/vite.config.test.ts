import { describe, expect, it } from 'vitest';

import config from './vite.config';

describe('development API proxy', () => {
  it('uses the selected Tellurion application origin for every UI API route', () => {
    const previous = process.env.TELLURION_APP_ORIGIN;
    process.env.TELLURION_APP_ORIGIN = 'http://127.0.0.1:18080';

    try {
      expect(typeof config).toBe('function');
      const resolved = config({
        command: 'serve',
        mode: 'development',
        isPreview: false,
        isSsrBuild: false,
      });
      const proxy = resolved.server?.proxy;

      expect(proxy).toEqual({
        '/public': 'http://127.0.0.1:18080',
        '/metrics': 'http://127.0.0.1:18080',
        '/_control': 'http://127.0.0.1:18080',
        '/_auth': 'http://127.0.0.1:18080',
        '/demo': 'http://127.0.0.1:18080',
      });
    } finally {
      if (previous === undefined) delete process.env.TELLURION_APP_ORIGIN;
      else process.env.TELLURION_APP_ORIGIN = previous;
    }
  });
});
