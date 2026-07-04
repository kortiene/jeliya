import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import type { DaemonErrorShape } from '../lib/protocol';
import { colorForId, formatBytes, initials } from '../lib/format';
import { useNames } from './names';

// -- brand mark ---------------------------------------------------------------

let gradientSeq = 0;

export function HexMark({ size = 30 }: { size?: number }) {
  const [gid] = useState(() => `hexg-${gradientSeq++}`);
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" fill="none" aria-hidden="true">
      <path
        d="M16 2.5 27.5 9v14L16 29.5 4.5 23V9L16 2.5Z"
        stroke={`url(#${gid})`}
        strokeWidth="2"
        strokeLinejoin="round"
      />
      <path
        d="M16 9.5 21.5 12.7v6.6L16 22.5l-5.5-3.2v-6.6L16 9.5Z"
        stroke={`url(#${gid})`}
        strokeWidth="1.4"
        strokeLinejoin="round"
        opacity="0.9"
      />
      <path d="M16 9.5v-7M21.5 19.3l6 3.7M10.5 19.3l-6 3.7" stroke={`url(#${gid})`} strokeWidth="1.2" opacity="0.55" />
      <defs>
        <linearGradient id={gid} x1="4" y1="4" x2="28" y2="28" gradientUnits="userSpaceOnUse">
          <stop stopColor="#3ee6b0" />
          <stop offset="1" stopColor="#1fb4a8" />
        </linearGradient>
      </defs>
    </svg>
  );
}

// -- avatars & names ----------------------------------------------------------

export function Avatar({ id, size = 34 }: { id: string; size?: number }) {
  const names = useNames();
  const label = names.isSelf(id) ? 'You' : names.display(id);
  const color = colorForId(id);
  return (
    <span
      className="avatar"
      style={{
        width: size,
        height: size,
        color,
        background: `${color}26`,
        fontSize: Math.max(10, Math.round(size * 0.34)),
      }}
      aria-hidden="true"
    >
      {initials(label)}
    </span>
  );
}

export function SenderName({ id, className = '' }: { id: string; className?: string }) {
  const names = useNames();
  // "You" is not renameable — plain text, not a dead button.
  if (names.isSelf(id)) {
    return (
      <span className={`sender-name ${className}`} title={id}>
        You
      </span>
    );
  }
  return (
    <button
      type="button"
      className={`sender-name ${className}`}
      title={`${id}\nClick to set a local name`}
      onClick={() => names.requestRename(id)}
    >
      {names.display(id)}
    </button>
  );
}

// -- small widgets --------------------------------------------------------------

export function ProgressBar({ value, label = 'Task progress' }: { value: number; label?: string }) {
  const v = Math.max(0, Math.min(100, value));
  return (
    <div className="progress" role="progressbar" aria-label={label} aria-valuenow={v} aria-valuemin={0} aria-valuemax={100}>
      <div className="progress-fill" style={{ width: `${v}%` }} />
    </div>
  );
}

export function CopyButton({ text, label = 'Copy', ariaLabel }: { text: string; label?: string; ariaLabel?: string }) {
  const [done, setDone] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const ta = document.createElement('textarea');
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand('copy');
      ta.remove();
    }
    setDone(true);
    window.setTimeout(() => setDone(false), 1400);
  };
  // aria-label only while idle — the visible "Copied ✓" swap must stay the
  // accessible name so the confirmation is announced.
  return (
    <button
      type="button"
      className="btn btn-ghost btn-sm"
      onClick={() => void copy()}
      aria-label={done ? undefined : ariaLabel}
    >
      {done ? 'Copied ✓' : label}
    </button>
  );
}

export function ErrorNote({ error }: { error: DaemonErrorShape | null }) {
  if (!error) return null;
  return (
    <div className="error-note" role="alert">
      <code className="error-code">{error.code}</code> {error.message}
      {error.hint ? <div className="error-hint">→ {error.hint}</div> : null}
    </div>
  );
}

export function Modal({
  title,
  onClose,
  children,
  wide = false,
}: {
  title: string;
  onClose(): void;
  children: ReactNode;
  wide?: boolean;
}) {
  const dialogRef = useRef<HTMLDivElement | null>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  // Real modal semantics for `aria-modal="true"`: Escape closes, Tab is trapped
  // inside the dialog (so focus never lands on the obscured room UI), and focus
  // returns to the element that opened the modal when it closes.
  useEffect(() => {
    const dialog = dialogRef.current;
    const previouslyFocused = document.activeElement;

    const focusables = (): HTMLElement[] => {
      if (!dialog) return [];
      const sel =
        'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';
      return Array.from(dialog.querySelectorAll<HTMLElement>(sel)).filter(
        (el) => el.offsetParent !== null || el === document.activeElement,
      );
    };

    // Initial focus: whatever React autofocused, else the first focusable, else
    // the dialog itself.
    if (dialog && !dialog.contains(document.activeElement)) {
      (focusables()[0] ?? dialog).focus();
    }

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onCloseRef.current();
        return;
      }
      if (e.key !== 'Tab' || !dialog) return;
      const items = focusables();
      if (items.length === 0) {
        e.preventDefault();
        dialog.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement as HTMLElement | null;
      if (e.shiftKey && (active === first || !dialog.contains(active))) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (active === last || !dialog.contains(active))) {
        e.preventDefault();
        first.focus();
      }
    };

    document.addEventListener('keydown', onKeyDown, true);
    return () => {
      document.removeEventListener('keydown', onKeyDown, true);
      if (previouslyFocused instanceof HTMLElement) previouslyFocused.focus();
    };
  }, []);

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className={`modal${wide ? ' modal-wide' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
      >
        <header className="modal-head">
          <h2>{title}</h2>
          <button type="button" className="icon-btn" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </header>
        <div className="modal-body">{children}</div>
      </div>
    </div>
  );
}

// -- fetch states (honest taxonomy, no invented delivery states) ---------------

export type FetchState =
  | { phase: 'pending' }
  | { phase: 'verified'; path: string; bytes: number }
  | { phase: 'error'; error: DaemonErrorShape };

export function FetchControl({ state, onFetch }: { state?: FetchState; onFetch(): void }) {
  if (!state) {
    return (
      <button type="button" className="btn btn-sm" onClick={onFetch}>
        Fetch
      </button>
    );
  }
  if (state.phase === 'pending') {
    return (
      <button type="button" className="btn btn-sm" disabled>
        <span className="spinner" aria-hidden="true" /> Fetching…
      </button>
    );
  }
  if (state.phase === 'verified') {
    return (
      <span className="fetch-ok" title={`verified · ${state.path}`}>
        ✓ Verified
      </span>
    );
  }
  // hash_mismatch is a hard stop per the protocol honesty rules — no retry.
  if (state.error.code === 'hash_mismatch') {
    return <span className="fetch-err">✕ Failed</span>;
  }
  return (
    <button type="button" className="btn btn-sm btn-danger" onClick={onFetch}>
      Retry
    </button>
  );
}

export function FetchDetail({ state }: { state?: FetchState }) {
  if (!state) return null;
  if (state.phase === 'verified') {
    return (
      <div className="fetch-detail ok">
        Verified · {formatBytes(state.bytes)} · saved to <code>{state.path}</code>
      </div>
    );
  }
  if (state.phase === 'error') {
    return (
      <div className="fetch-detail err">
        <code className="error-code">{state.error.code}</code> {state.error.message}
        {state.error.hint ? ` — ${state.error.hint}` : ''}
      </div>
    );
  }
  return null;
}
