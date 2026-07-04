import { useState } from 'react';
import type { DaemonErrorShape } from '../lib/protocol';
import { errorShape } from '../lib/protocol';
import { ErrorNote } from './ui';

export function Composer({
  roomName,
  disabled,
  onSend,
}: {
  roomName: string;
  disabled: boolean;
  onSend(body: string): Promise<void>;
}) {
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<DaemonErrorShape | null>(null);

  const send = async () => {
    const body = draft.trim();
    if (!body || sending || disabled) return;
    setSending(true);
    setError(null);
    try {
      await onSend(body);
      setDraft('');
    } catch (e) {
      setError(errorShape(e));
    } finally {
      setSending(false);
    }
  };

  return (
    <div className="composer">
      <ErrorNote error={error} />
      <div className="composer-bar">
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              void send();
            }
          }}
          placeholder={`Message ${roomName}`}
          aria-label={`Message ${roomName}`}
          rows={1}
          disabled={disabled}
        />
        <button
          type="button"
          className="btn btn-primary composer-send"
          onClick={() => void send()}
          disabled={disabled || sending || !draft.trim()}
          aria-label="Send message"
        >
          {sending ? '…' : '➤'}
        </button>
      </div>
      <div className="composer-hint muted">Enter to send · Shift+Enter for a new line</div>
    </div>
  );
}
