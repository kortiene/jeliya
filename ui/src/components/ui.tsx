import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import type { DaemonErrorShape } from '../lib/protocol';
import { colorForId, formatBytes, initials } from '../lib/format';
import { friendlyError } from '../lib/errors';
import { useNames } from './names';

// -- brand mark ---------------------------------------------------------------
//
// The meeting tree: a canopy, a trunk, and three peers gathered under it —
// the village tree where the jeli (whose art is jeliya: keeping the
// community's true record) speaks to the gathered community. Flat
// single-accent stroke only (PRODUCT.md forbids gradient text, glow, and
// neon hexagons); the dots reuse the presence-dot vocabulary.

export function TreeMark({ size = 30 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 32 32"
      fill="none"
      aria-hidden="true"
      style={{ color: 'var(--accent)' }}
    >
      <path d="M7 15 A9 9 0 0 1 25 15" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" />
      <path d="M16 13.5 V22" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round" />
      <circle cx="9.5" cy="24.5" r="1.7" fill="currentColor" />
      <circle cx="16" cy="26.5" r="1.7" fill="currentColor" />
      <circle cx="22.5" cy="24.5" r="1.7" fill="currentColor" />
    </svg>
  );
}

// The wordmark: "Jeliya" in the display stack, weight 700, 0.01em tracking,
// ink — never emerald (the mark carries the accent; a green wordmark would
// spend signal on decoration). Size comes from the surrounding context class.
export function Wordmark({ as: Tag = 'span', className }: { as?: 'span' | 'h1'; className?: string }) {
  return <Tag className={className ? `wordmark ${className}` : 'wordmark'}>Jeliya</Tag>;
}

// -- avatars & names ----------------------------------------------------------

export function Avatar({ id, size = 34 }: { id: string; size?: number }) {
  const names = useNames();
  // Self resolves to its device-local label (or "You") like everyone else.
  const label = names.display(id);
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
  // Self shows its device-local label (or "You"); it is renamed from
  // onboarding/settings, not inline — plain text, not a dead button.
  if (names.isSelf(id)) {
    return (
      <span className={`sender-name ${className}`} title={id}>
        {names.display(id)}
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

/** The editable, device-local self label (docs/self-label.md). Holds its own
 *  input state so trimming on persist never fights mid-word spaces; the parent
 *  writes it to the local alias store. Empty clears the label back to "You". */
export function SelfLabelField({
  value,
  onChange,
  autoFocus = false,
}: {
  value: string;
  onChange(next: string): void;
  autoFocus?: boolean;
}) {
  const [text, setText] = useState(value);
  return (
    <label className="field">
      <span>
        Your name on this device <em className="muted">(local only)</em>
      </span>
      <input
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          onChange(e.target.value);
        }}
        placeholder="e.g. Alex"
        maxLength={40}
        autoFocus={autoFocus}
        aria-label="Your name on this device"
      />
      <span className="field-hint muted">Only visible to you — never shared or signed.</span>
    </label>
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
  const friendly = friendlyError(error);
  return (
    <div className="error-note" role="alert">
      <strong className="error-title">{friendly.title}</strong>
      <span>{friendly.message}</span>
      {friendly.action ? <div className="error-hint">{friendly.action}</div> : null}
      <details className="error-details">
        <summary>Technical details</summary>
        <code className="error-code">{error.code}</code> {error.message}
        {error.hint ? <div>{error.hint}</div> : null}
      </details>
    </div>
  );
}

export function Modal({
  title,
  onClose,
  children,
  wide = false,
  busy = false,
}: {
  title: string;
  onClose(): void;
  children: ReactNode;
  wide?: boolean;
  /** A non-cancellable operation is in flight: Escape, backdrop, and the ✕
   *  cannot dismiss the dialog until it settles. Dismissal would only hide
   *  the request — its result would still mutate state after the user
   *  believed the action was abandoned. */
  busy?: boolean;
}) {
  const dialogRef = useRef<HTMLDivElement | null>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const busyRef = useRef(busy);
  busyRef.current = busy;

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
        if (!busyRef.current) onCloseRef.current();
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
        if (e.target === e.currentTarget && !busy) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className={`modal${wide ? ' modal-wide' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        aria-busy={busy || undefined}
        tabIndex={-1}
      >
        <header className="modal-head">
          <h2>{title}</h2>
          <button type="button" className="icon-btn" onClick={onClose} aria-label="Close" disabled={busy}>
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
  | { phase: 'verified'; path: string; bytes: number; url?: string }
  | { phase: 'fetched'; path: string; bytes: number; url?: string }
  | { phase: 'error'; error: DaemonErrorShape };

export interface FetchAvailability {
  available: boolean;
  providers: number;
}

function providerTitle(availability?: FetchAvailability): string | undefined {
  if (!availability) return undefined;
  const providers = `${availability.providers} provider${availability.providers === 1 ? '' : 's'} listed`;
  return availability.available ? `${providers}; at least one is online` : `${providers}; none are online right now`;
}

export function FetchControl({
  state,
  availability,
  availabilityPending = false,
  onFetch,
  onRecheck,
}: {
  state?: FetchState;
  availability?: FetchAvailability;
  availabilityPending?: boolean;
  onFetch(): void;
  onRecheck?(): void;
}) {
  if (!state) {
    if (availabilityPending) {
      return (
        <button type="button" className="btn btn-sm" disabled>
          <span className="spinner" aria-hidden="true" /> Checking…
        </button>
      );
    }
    if (availability && !availability.available) {
      return (
        <span className="fetch-actions" title={providerTitle(availability)}>
          <span className="fetch-offline">No provider online</span>
          {onRecheck ? (
            <button type="button" className="btn btn-sm btn-ghost" onClick={onRecheck}>
              Recheck
            </button>
          ) : null}
        </span>
      );
    }
    return (
      <button type="button" className="btn btn-sm" onClick={onFetch} title={providerTitle(availability)}>
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
  if (state.phase === 'verified' || state.phase === 'fetched') {
    return (
      <span className="fetch-actions">
        {state.url ? (
          <a className="btn btn-sm btn-primary" href={state.url} target="_blank" rel="noreferrer">
            Open file
          </a>
        ) : (
          <span className="fetch-ok" title={`${state.phase === 'verified' ? 'verified' : 'fetched'} · ${state.path}`}>
            {state.phase === 'verified' ? '✓ Verified' : '✓ Fetched'}
          </span>
        )}
        <CopyButton text={state.path} label="Copy path" ariaLabel="Copy saved file path" />
      </span>
    );
  }
  // hash_mismatch is a hard stop per the protocol honesty rules — no retry.
  if (state.error.code === 'hash_mismatch') {
    return <span className="fetch-err">✕ Failed</span>;
  }
  if (availability && !availability.available) {
    return (
      <span className="fetch-actions" title={providerTitle(availability)}>
        <span className="fetch-offline">No provider online</span>
        {onRecheck ? (
          <button type="button" className="btn btn-sm btn-ghost" onClick={onRecheck}>
            Recheck
          </button>
        ) : null}
      </span>
    );
  }
  return (
    <button type="button" className="btn btn-sm btn-danger" onClick={onFetch}>
      Retry
    </button>
  );
}

function fetchErrorCopy(error: DaemonErrorShape): { message: string; detail?: string } {
  switch (error.code) {
    case 'file_unavailable':
      return {
        message: 'No provider is online for this file yet. Recheck when the sender is back online.',
        detail: error.hint ?? error.message,
      };
    case 'file_unauthorized':
      return {
        message: 'Every provider refused this fetch — your identity is not authorized for it. Ask the sender to re-share or re-invite you.',
        detail: error.hint ?? error.message,
      };
    default:
      return {
        message: error.message,
        detail: error.hint ?? undefined,
      };
  }
}

export function FetchDetail({ state }: { state?: FetchState }) {
  if (!state) return null;
  if (state.phase === 'verified' || state.phase === 'fetched') {
    const path = state.url ? (
      <a className="fetch-path-link" href={state.url} target="_blank" rel="noreferrer" title="Open local file copy">
        <code>{state.path}</code>
      </a>
    ) : (
      <code>{state.path}</code>
    );
    return (
      <div className="fetch-detail ok">
        {state.phase === 'verified' ? 'Verified' : 'Fetched'} · {formatBytes(state.bytes)} · saved to{' '}
        {path}
      </div>
    );
  }
  if (state.phase === 'error') {
    // hash_mismatch means a real integrity-check failure: the file was fetched but
    // its content didn't match the expected hash, so it was discarded. Lead with
    // plain language — the raw code/message/hint is real BLAKE3-hash-and-security
    // language that means nothing to a non-developer. Keep it, but de-emphasized.
    if (state.error.code === 'hash_mismatch') {
      return (
        <div className="fetch-detail err">
          This file failed a security check and wasn't saved — it may have been
          corrupted or tampered with in transit.
          <details className="fetch-detail-advanced">
            <summary className="muted">Technical details</summary>
            <code className="error-code">{state.error.code}</code> {state.error.message}
            {state.error.hint ? ` — ${state.error.hint}` : ''}
          </details>
        </div>
      );
    }
    const copy = fetchErrorCopy(state.error);
    return (
      <div className="fetch-detail err">
        {copy.message}
        {copy.detail ? (
          <details className="fetch-detail-advanced">
            <summary className="muted">Technical details</summary>
            <code className="error-code">{state.error.code}</code> {copy.detail}
          </details>
        ) : null}
      </div>
    );
  }
  return null;
}
