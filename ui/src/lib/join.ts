import type { Client, DaemonErrorShape } from './protocol';
import { errorShape, RequestError } from './protocol';

export type JoinPhase = 'connecting' | 'retrying';

export interface JoinProgress {
  phase: JoinPhase;
  attempt: number;
  maxAttempts: number;
  message: string;
  lastError?: DaemonErrorShape;
}

const JOIN_ATTEMPTS = 5;
const RETRY_DELAYS_MS = [1_500, 2_000, 3_000, 4_000];

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

export async function joinRoomWithRetry(
  client: Client,
  params: { ticket: string; peers?: string[] },
  onProgress?: (progress: JoinProgress) => void,
): Promise<{ room_id: string }> {
  let lastError: unknown = null;

  for (let attempt = 1; attempt <= JOIN_ATTEMPTS; attempt += 1) {
    onProgress?.({
      phase: 'connecting',
      attempt,
      maxAttempts: JOIN_ATTEMPTS,
      message:
        attempt === 1
          ? 'Finding the inviter and syncing the room invite...'
          : `Retrying join (${attempt}/${JOIN_ATTEMPTS})...`,
    });

    try {
      return await client.call('room.join', params);
    } catch (e) {
      lastError = e;
      const err = errorShape(e);
      if (err.code !== 'peer_unreachable' || attempt === JOIN_ATTEMPTS) {
        throw e;
      }
      const delay = RETRY_DELAYS_MS[Math.min(attempt - 1, RETRY_DELAYS_MS.length - 1)];
      onProgress?.({
        phase: 'retrying',
        attempt,
        maxAttempts: JOIN_ATTEMPTS,
        message: `The first path did not answer. Retrying in ${Math.round(delay / 1000)}s...`,
        lastError: err,
      });
      await sleep(delay);
      // The daemon connection can drop during the backoff. Issuing the next
      // attempt then would QUEUE it until reconnect (WsClient keeps unsent
      // requests), pinning the caller's busy state — and the contained join
      // dialog with it — for as long as the daemon stays down. Fail honestly
      // instead: the dialog surfaces the real error and dismissal returns.
      if (client.getState() !== 'connected') {
        throw new RequestError({
          code: 'connection_lost',
          message: 'the daemon connection dropped while waiting to retry the join',
          hint: 'reconnect, then paste the same invite again',
        });
      }
    }
  }

  throw lastError;
}
