// @vitest-environment jsdom
//
// joinRoomWithRetry retries only peer_unreachable — and must abort honestly
// if the daemon connection drops during a retry backoff: issuing the next
// attempt while disconnected would queue it until reconnect, pinning the
// caller's busy state (and the contained join dialog) indefinitely.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { joinRoomWithRetry } from './join';
import type { Client, ConnectionState } from './protocol';
import { RequestError } from './protocol';

function fakeClient(overrides: {
  call: Client['call'];
  getState?: () => ConnectionState;
}): Client {
  return {
    describe: () => 'fake',
    start: () => undefined,
    stop: () => undefined,
    getState: overrides.getState ?? (() => 'connected'),
    onState: () => () => undefined,
    on: () => () => undefined,
    call: overrides.call,
  };
}

const peerUnreachable = () =>
  new RequestError({ code: 'peer_unreachable', message: 'no path to inviter', hint: null });

describe('joinRoomWithRetry', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('retries peer_unreachable and succeeds while the connection holds', async () => {
    let calls = 0;
    const client = fakeClient({
      call: (async () => {
        calls += 1;
        if (calls === 1) throw peerUnreachable();
        return { room_id: 'room-1' };
      }) as Client['call'],
    });

    const result = joinRoomWithRetry(client, { ticket: 'roomtkt1'.padEnd(24, 'x') });
    await vi.advanceTimersByTimeAsync(2_000);
    await expect(result).resolves.toEqual({ room_id: 'room-1' });
    expect(calls).toBe(2);
  });

  it('aborts with connection_lost if the daemon drops during the backoff', async () => {
    let calls = 0;
    let state: ConnectionState = 'connected';
    const client = fakeClient({
      getState: () => state,
      call: (async () => {
        calls += 1;
        throw peerUnreachable();
      }) as Client['call'],
    });

    const result = joinRoomWithRetry(client, { ticket: 'roomtkt1'.padEnd(24, 'x') });
    // Guard against an unhandled-rejection blip while timers advance.
    const settled = result.catch((e: unknown) => e);
    state = 'reconnecting'; // the daemon goes away while we wait to retry
    await vi.advanceTimersByTimeAsync(2_000);

    const error = await settled;
    expect(error).toBeInstanceOf(RequestError);
    expect((error as RequestError).code).toBe('connection_lost');
    // The queued-forever second attempt never fires.
    expect(calls).toBe(1);
  });
});
