import { defineConfig } from '@playwright/test';

export default defineConfig({
    testDir: './e2e',
    timeout: 15_000,
    use: {
        baseURL: 'http://localhost:5812',
        // editor-server serves no TLS
        ignoreHTTPSErrors: false,
    },
    // Tests run sequentially so scene state is predictable across cases.
    workers: 1,
});
