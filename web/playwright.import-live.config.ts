import { defineConfig } from '@playwright/test'

export default defineConfig({
  testDir: './tests',
  testMatch: 'import-live.spec.ts',
  workers: 1,
  timeout: 180_000,
  use: {
    baseURL: 'http://127.0.0.1:4187',
    channel: process.env.PLAYWRIGHT_CHANNEL,
    headless: true,
    actionTimeout: 15_000,
    trace: 'off',
    screenshot: 'off',
    video: 'off',
  },
  webServer: {
    command: 'cargo run --quiet --manifest-path ..\\Cargo.toml -p azbridge-broker -- --dev --listen-addr 127.0.0.1:4187 --public-url http://127.0.0.1:4187 --static-dir dist',
    url: 'http://127.0.0.1:4187/healthz',
    timeout: 120_000,
    reuseExistingServer: false,
  },
})
