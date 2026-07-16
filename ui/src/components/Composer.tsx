import { useCallback, useEffect, useRef, useState } from 'react';
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

  const autosize = useCallback(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = '0px';
    if (el.scrollHeight === 0) {
      // Inside a display:none pane (compact keeps inactive panes unlaid-out)
      // every measurement reads 0 — writing that would clip the composer to a
      // strip. Fall back to the stylesheet's one-line height; the observer
      // below re-measures the moment the pane is actually laid out.
      el.style.height = '';
      return;
    }
    el.style.height = `${Math.min(el.scrollHeight, 150)}px`;
  }, []);

  useEffect(() => {
    autosize();
  }, [draft, autosize]);

  // Re-measure when the textarea's laid-out width changes: the hidden→visible
  // transition (0→w), viewport resize, and rotation. Keying on width ignores
  // the height changes autosize itself causes, so the observer cannot loop.
  const lastWidth = useRef(-1);
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    const observer = new ResizeObserver((entries) => {
      const width = entries[entries.length - 1].contentRect.width;
      if (width === lastWidth.current) return;
      lastWidth.current = width;
      if (width > 0) autosize();
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [autosize]);

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
