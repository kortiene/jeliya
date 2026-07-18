import { useCallback, useEffect, useRef, useState } from 'react';
import type { DaemonErrorShape } from '../lib/protocol';
import { errorShape } from '../lib/protocol';
import { useStrings } from '../l10n/strings';
import { Glyph } from '../l10n/tokens';
import { ErrorNote } from './ui';

export function Composer({
  roomId,
  roomName,
  disabled,
  compact,
  onSend,
  onShareFile,
}: {
  roomId: string;
  roomName: string;
  disabled: boolean;
  /** Compact widths get the touch-composer behavior (web parity with the
   *  Flutter composer): Enter inserts a newline and the ➤ button is the
   *  explicit send, so the "Enter to send" hint — false there — is withheld. */
  compact: boolean;
  onSend(body: string): Promise<void>;
  onShareFile(file: File): Promise<void>;
}) {
  const s = useStrings();
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [sharing, setSharing] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState<DaemonErrorShape | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const attachRef = useRef<HTMLInputElement | null>(null);
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
          // The "skip to message composer" link's target (App.tsx). Reaching
          // the composer otherwise means tabbing the entire timeline.
          id="composer-input"
          value={draft}
          onChange={(e) => updateDraft(e.target.value)}
          onPaste={(e) => {
            if (e.clipboardData.files.length === 0) return;
            e.preventDefault();
            void shareFiles(e.clipboardData.files);
          }}
          onKeyDown={(e) => {
            // Desktop: Enter sends, Shift+Enter is a newline. Compact: Enter is
            // always a newline and the ➤ button is the explicit send (the soft
            // keyboard's own newline key), so this handler stands down there.
            if (!compact && e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              void send();
            }
          }}
          placeholder={s.composerMessagePlaceholder(roomName)}
          aria-label={s.composerMessagePlaceholder(roomName)}
          rows={1}
          disabled={disabled}
        />
        {/* Touch-native attachment (#67 P20), the counterpart to paste/drop for
            devices that have neither. Wired to the same verified share flow;
            disabled only by `sharing`, never by `sending` — a share in flight
            must never block a send, nor the reverse. */}
        <input
          ref={attachRef}
          type="file"
          className="composer-attach-input"
          onChange={(e) => {
            const picked = e.currentTarget.files;
            if (picked && picked.length > 0) void shareFiles(picked);
            e.currentTarget.value = '';
          }}
          aria-hidden="true"
          tabIndex={-1}
        />
        <button
          type="button"
          className="icon-btn composer-attach"
          onClick={() => attachRef.current?.click()}
          disabled={disabled || sharing}
          aria-label={s.composerShareAFile}
        >
          {sharing ? <span className="spinner" aria-hidden="true" /> : <span aria-hidden="true">{Glyph.file}</span>}
        </button>
        <button
          type="button"
          className="btn btn-primary composer-send"
          onClick={() => void send()}
          disabled={disabled || sending || !draft.trim()}
          aria-label={s.composerSendMessage}
        >
          {sending ? '…' : Glyph.send}
        </button>
      </div>
      {/* The hint describes desktop Enter behavior; on compact that claim is
          false (Enter is a newline), so the line is withheld there unless it is
          carrying live sharing feedback. */}
      {sharing || !compact ? (
        <div className="composer-hint muted">
          {sharing ? s.composerSharingFile : s.composerHint}
        </div>
      ) : null}
    </div>
  );
}
