import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import type { DaemonErrorShape } from '../lib/protocol';
import { colorForId, initials } from '../lib/format';
import type { Catalog } from '../l10n/catalog';
import { friendlyError } from '../l10n/errorDisplay';
import { useFormats, useStrings } from '../l10n/strings';
import { Template } from '../l10n/template';
import { BRAND, Glyph, Punct } from '../l10n/tokens';
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
  return <Tag className={className ? `wordmark ${className}` : 'wordmark'}>{BRAND}</Tag>;
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
  const s = useStrings();
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
      title={s.commonSetLocalNameFor(id)}
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
  const s = useStrings();
  return (
    <label className="field">
      <span>{s.selfLabelTitle}</span>
      <input
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          onChange(e.target.value);
        }}
        placeholder={s.selfLabelPlaceholder}
        maxLength={40}
        autoFocus={autoFocus}
        aria-label={s.selfLabelTitle}
      />
      <span className="field-hint muted">{s.selfLabelHint}</span>
    </label>
  );
}

// -- small widgets --------------------------------------------------------------

export function ProgressBar({ value, label }: { value: number; label?: string }) {
  const v = Math.max(0, Math.min(100, value));
  const s = useStrings();
  return (
    <div
      className="progress"
      role="progressbar"
      aria-label={label ?? s.commonTaskProgress}
      aria-valuenow={v}
      aria-valuemin={0}
      aria-valuemax={100}
    >
      <div className="progress-fill" style={{ width: `${v}%` }} />
    </div>
  );
}

export function CopyButton({ text, label, ariaLabel }: { text: string; label?: string; ariaLabel?: string }) {
  const [done, setDone] = useState(false);
  const s = useStrings();
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
      {done ? s.commonCopied : (label ?? s.commonCopy)}
    </button>
  );
}

export function ErrorNote({ error }: { error: DaemonErrorShape | null }) {
  const s = useStrings();
  if (!error) return null;
  const friendly = friendlyError(s, error);
  return (
    <div className="error-note" role="alert">
      <strong className="error-title">{friendly.title}</strong>
      <span>{friendly.message}</span>
      {friendly.action ? <div className="error-hint">{friendly.action}</div> : null}
      <details className="error-details">
        <summary>{s.commonTechnicalDetails}</summary>
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
  const s = useStrings();
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
          {/* Deliberately just "Close" (the ARIA dialog pattern's own name for
              it), not "Close <title>". Exactly one dialog is ever open, and the
              dialog announces its own name on entry, so the short name is
              unambiguous where it is heard. Folding the title in also makes the
              close button match any search for the title itself — including an
              AT user's find-by-name — which is worse, not better. */}
          <button type="button" className="icon-btn" onClick={onClose} aria-label={s.commonClose} disabled={busy}>
            {Glyph.close}
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

function providerTitle(s: Catalog, formats: ReturnType<typeof useFormats>, availability?: FetchAvailability): string | undefined {
  if (!availability) return undefined;
  return availability.available
    ? s.fetchProvidersListedOnline(availability.providers, formats.count(availability.providers))
    : s.fetchProvidersListedOffline(availability.providers, formats.count(availability.providers));
}

export function FetchControl({
  state,
  availability,
  availabilityPending = false,
  fileName,
  onFetch,
  onRecheck,
}: {
  state?: FetchState;
  availability?: FetchAvailability;
  availabilityPending?: boolean;
  /** The file this control acts on. Many files render side by side, each
   *  producing a control whose visible text is the same single word, so the
   *  name joins the ACCESSIBLE name to keep "Fetch, Fetch, Fetch" from being
   *  all a screen-reader user hears (issue #72). The visible label is
   *  deliberately left short — the file name is already beside it. */
  fileName?: string;
  onFetch(): void;
  onRecheck?(): void;
}) {
  const s = useStrings();
  const formats = useFormats();
  // Undefined when no name was passed, which leaves the visible text as the
  // accessible name — the correct fallback, never an invented one.
  if (!state) {
    if (availabilityPending) {
      return (
        <button type="button" className="btn btn-sm" disabled>
          <span className="spinner" aria-hidden="true" /> {s.commonChecking}
        </button>
      );
    }
    if (availability && !availability.available) {
      return (
        <span className="fetch-actions" title={providerTitle(s, formats, availability)}>
          <span className="fetch-offline">{s.commonNoProviderOnline}</span>
          {onRecheck ? (
            <button
              type="button"
              className="btn btn-sm btn-ghost"
              onClick={onRecheck}
              aria-label={fileName ? s.fetchRecheckProvidersFor(fileName) : undefined}
            >
              {s.commonRecheck}
            </button>
          ) : null}
        </span>
      );
    }
    return (
      <button
        type="button"
        className="btn btn-sm"
        onClick={onFetch}
        title={providerTitle(s, formats, availability)}
        aria-label={fileName ? s.fetchFileNamed(fileName) : undefined}
      >
        {s.commonFetch}
      </button>
    );
  }
  if (state.phase === 'pending') {
    return (
      <button type="button" className="btn btn-sm" disabled>
        <span className="spinner" aria-hidden="true" /> {s.commonFetching}
      </button>
    );
  }
  if (state.phase === 'verified' || state.phase === 'fetched') {
    return (
      <span className="fetch-actions">
        {state.url ? (
          <a
            className="btn btn-sm btn-primary"
            href={state.url}
            target="_blank"
            rel="noreferrer"
            // Starts with the VISIBLE label. WCAG 2.5.3 Label in Name requires
            // the accessible name to contain the visible text, or a
            // speech-input user saying "click Open file" matches nothing.
            aria-label={fileName ? s.fetchOpenFileNamed(fileName) : undefined}
          >
            {s.commonOpenFile}
          </a>
        ) : (
          <span
            className="fetch-ok"
            title={state.phase === 'verified' ? s.fetchVerifiedTooltip(state.path) : s.fetchFetchedTooltip(state.path)}
          >
            {state.phase === 'verified' ? s.commonVerified : s.commonFetched}
          </span>
        )}
        <CopyButton
          text={state.path}
          label={s.commonCopyPath}
          ariaLabel={fileName ? s.fetchCopySavedPathFor(fileName) : s.commonCopySavedFilePath}
        />
      </span>
    );
  }
  // hash_mismatch is a hard stop per the protocol honesty rules — no retry.
  if (state.error.code === 'hash_mismatch') {
    return <span className="fetch-err">{s.commonFailed}</span>;
  }
  if (availability && !availability.available) {
    return (
      <span className="fetch-actions" title={providerTitle(s, formats, availability)}>
        <span className="fetch-offline">{s.commonNoProviderOnline}</span>
        {onRecheck ? (
          <button
            type="button"
            className="btn btn-sm btn-ghost"
            onClick={onRecheck}
            aria-label={fileName ? s.fetchRecheckProvidersFor(fileName) : undefined}
          >
            {s.commonRecheck}
          </button>
        ) : null}
      </span>
    );
  }
  return (
    <button
      type="button"
      className="btn btn-sm btn-danger"
      onClick={onFetch}
      aria-label={fileName ? s.fetchRetryNamed(fileName) : undefined}
    >
      {s.commonRetry}
    </button>
  );
}

function technicalErrorDetail(error: DaemonErrorShape): string {
  return [error.message, error.hint].filter((part): part is string => Boolean(part)).join(Punct.metaSep);
}

function fetchErrorCopy(s: Catalog, error: DaemonErrorShape): { message: string; detail: string } {
  switch (error.code) {
    case 'file_unavailable':
      return {
        message: s.fetchErrFileUnavailable,
        detail: technicalErrorDetail(error),
      };
    case 'file_unauthorized':
      return {
        message: s.fetchErrFileUnauthorized,
        detail: technicalErrorDetail(error),
      };
    default:
      return {
        message: friendlyError(s, error).message,
        detail: technicalErrorDetail(error),
      };
  }
}

export function FetchDetail({ state }: { state?: FetchState }) {
  const s = useStrings();
  const formats = useFormats();
  if (!state) return null;
  if (state.phase === 'verified' || state.phase === 'fetched') {
    const path = state.url ? (
      <a
        className="fetch-path-link"
        href={state.url}
        target="_blank"
        rel="noreferrer"
        title={s.fetchOpenLocalFileCopy}
      >
        <code>{state.path}</code>
      </a>
    ) : (
      <code>{state.path}</code>
    );
    return (
      <div className="fetch-detail ok">
        <Template
          template={state.phase === 'verified' ? s.fetchDetailVerified : s.fetchDetailFetched}
          slots={{ bytes: formats.bytes(state.bytes), path }}
        />
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
          {s.fetchErrHashMismatch}
          <details className="fetch-detail-advanced">
            <summary className="muted">{s.commonTechnicalDetails}</summary>
            <code className="error-code">{state.error.code}</code> {state.error.message}
            {state.error.hint ? ` — ${state.error.hint}` : ''}
          </details>
        </div>
      );
    }
    const copy = fetchErrorCopy(s, state.error);
    return (
      <div className="fetch-detail err">
        {copy.message}
        {copy.detail ? (
          <details className="fetch-detail-advanced">
            <summary className="muted">{s.commonTechnicalDetails}</summary>
            <code className="error-code">{state.error.code}</code> {copy.detail}
          </details>
        ) : null}
      </div>
    );
  }
  return null;
}
