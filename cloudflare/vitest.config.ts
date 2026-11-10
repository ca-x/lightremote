import { cloudflareTest } from '@cloudflare/vitest-plugin';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  plugins: [cloudflareTest({
    wrangler: { configPath: './wrangler.toml' },
    miniflare: { bindings: { ROC_REMOTE_RELAY_TOKEN: 'test-operator-secret', ROC_REMOTE_SESSION_BYTES: '300000', ROC_REMOTE_MAX_ROOMS: '8' } },
  })],
});
