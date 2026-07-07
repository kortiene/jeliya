import { useEffect, useRef, useState } from 'react';
import type { DaemonErrorShape } from '../lib/protocol';
import { errorShape } from '../lib/protocol';
import { ErrorNote } from './ui';

export function Composer({
  roomId,
  roomName,
  disabled,
  onSend,
  onShareFile,
}: {
  roomId: string;
  roomName: string;
  disabled: boolean;
  onSend(body: string): Promise<void>;
  onShareFile(file: File): Promise<void>;
}) {
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [sharing, setSharing] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState<DaemonErrorShape | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const draftKey = `jeliya.draft.${roomId}`;

  useEffect(() => {
    try {
      setDraft(localStorage.getItem(draftKey) ?? '');
    } catch {
      setDraft('');
    }
  }, [draftKey]);

  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = '0px';
    el.style.height = `${Math.min(el.scrollHeight, 150)}px`;
  }, [draft]);

  const updateDraft = (value: string) => {
    setDraft(value);
    try {
      if (value) localStorage.setItem(draftKey, value);
      else localStorage.removeItem(draftKey);
    } catch {
      /* ignore */
    }
  };

  const send = async () => {
    const body = draft.trim();
    if (!body || sending || disabled) return;
    const previousDraft = draft;
    updateDraft('');
    setSending(true);
    setError(null);
    try {
      await onSend(body);
    } catch (e) {
      updateDraft(previousDraft);
      setError(errorShape(e));
    } finally {
      setSending(false);
    }
  };

  const shareFiles = async (files: FileList | File[]) => {
    const picked = Array.from(files).filter((file) => file.size > 0);
    if (picked.length === 0 || disabled || sharing) return;
    setSharing(true);
    setError(null);
    try {
      for (const file of picked) {
        await onShareFile(file);
      }
    } catch (e) {
      setError(errorShape(e));
    } finally {
      setSharing(false);
      setDragging(false);
    }
  };

  return (
    <div className="composer">
      <ErrorNote error={error} />
      <div
        className={`composer-bar${dragging ? ' is-dragging' : ''}`}
        onDragOver={(e) => {
          if (disabled) return;
          e.preventDefault();
          setDragging(true);
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={(e) => {
          if (disabled) return;
          e.preventDefault();
          void shareFiles(e.dataTransfer.files);
        }}
      >
        <textarea
          ref={textareaRef}
          value={draft}
          onChange={(e) => updateDraft(e.target.value)}
          onPaste={(e) => {
            if (e.clipboardData.files.length === 0) return;
            e.preventDefault();
            void shareFiles(e.clipboardData.files);
          }}
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
      <div className="composer-hint muted">
        {sharing ? 'Sharing file…' : 'Enter to send · Shift+Enter for a new line · Paste or drop a file to share'}
      </div>
    </div>
  );
}
