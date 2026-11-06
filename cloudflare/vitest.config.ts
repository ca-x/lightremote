import { cloudflareTest } from '@cloudflare/vitest-plugin';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [cloudflareTest({
    wrangler: { configPath: './wrangler.toml' },
    miniflare: { bindings: { LIGHTREMOTE_RELAY_TOKEN: 'test-operator-secret', LIGHTREMOTE_SESSION_BYTES: '300000', LIGHTREMOTE_MAX_ROOMS: '8' } },
  })],
});
