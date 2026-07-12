// @vitest-environment jsdom
//
// Conformance corpus replayed against the in-memory mock reference client.
// jsdom supplies the `window` timers the mock uses. The daemon oracle runs the
// same corpus in conformance.daemon.test.ts.

import { beforeAll, describe, expect, it } from 'vitest';

import { createMockClient } from '../mock';
import type { Client } from '../protocol';
import { replayScenario, type Scenario } from './harness';
import corpus from './corpus.json';

const scenarios = (corpus.scenarios as Scenario[]).filter((s) => !s.tags?.includes('daemonOnly'));

describe('conformance: mock reference client', () => {
  let client: Client;

  beforeAll(async () => {
    client = createMockClient();
    client.start();
    // Shared precondition: an identity exists. The mock is seeded with one, so
    // identity.create would error identity_exists — tolerate that.
    await client.call('identity.create', {}).catch(() => undefined);
    // Give the mock's simulated connect (200ms) time to settle.
    await new Promise((r) => setTimeout(r, 250));
  });

  for (const scenario of scenarios) {
    it(scenario.name, async () => {
      const preIdentity = scenario.tags?.includes('preIdentity') ?? false;
      let oracle = client;
      if (preIdentity) {
        const original = `${window.location.pathname}${window.location.search}${window.location.hash}`;
        window.history.replaceState({}, '', `${window.location.pathname}?mock=fresh`);
        oracle = createMockClient();
        window.history.replaceState({}, '', original);
        oracle.start();
        await new Promise((r) => setTimeout(r, 250));
      }
      try {
        const results = await replayScenario(oracle, scenario, 1500);
        const failures = results.filter((r) => !r.ok);
        expect(failures, JSON.stringify(failures, null, 2)).toEqual([]);
      } finally {
        if (preIdentity) oracle.stop();
      }
    });
  }
});
