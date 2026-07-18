import { useMemo } from 'react';
import { encodeQr } from '../lib/qr';

/** Quiet-zone modules on every side — scanners need this margin. */
const QUIET = 4;

/** Renders `value` as a scannable QR code, or nothing when the payload is too
 *  large for any symbol (the caller keeps Copy/Share as the fallback). The
 *  encoder is fully self-contained (ui/src/lib/qr.ts) — no runtime dependency,
 *  no CDN.
 *
 *  A QR is always dark-on-light with a 4-module quiet zone: scanners rely on
 *  that contrast and margin, so — unlike the rest of the UI — it does NOT invert
 *  in dark mode. The white plate is part of the drawing. The raw invite stays
 *  reachable via the adjacent Copy button, so this is an affordance, not the
 *  only path (aria-label names it; the modules themselves are decorative). */
export function QrCode({
  value,
  size = 208,
  label,
  caption,
}: {
  value: string;
  size?: number;
  label: string;
  /** Optional visible caption; rendered with the code inside one <figure> so it
   *  never appears without the image (both are dropped when encoding fails). */
  caption?: string;
}) {
  const qr = useMemo(() => encodeQr(value), [value]);
  // Both the encode and the path string are memoized on the value: InviteModal
  // re-renders once a SECOND while a time-boxed ticket counts down, and neither
  // is cheap. Dark modules are coalesced into one subpath per horizontal run
  // (the Flutter painter does the same), which keeps `d` small on big symbols.
  const d = useMemo(() => {
    if (!qr) return '';
    let out = '';
    const n = qr.size;
    for (let r = 0; r < n; r++) {
      const row = qr.modules[r];
      let c = 0;
      while (c < n) {
        if (!row[c]) {
          c++;
          continue;
        }
        const start = c;
        while (c < n && row[c]) c++;
        const len = c - start;
        out += `M${start + QUIET} ${r + QUIET}h${len}v1h-${len}z`;
      }
    }
    return out;
  }, [qr]);
  if (!qr) return null;

  const dim = qr.size + QUIET * 2;
  return (
    <figure className="invite-qr">
      <svg
        className="qr-code"
        width={size}
        height={size}
        viewBox={`0 0 ${dim} ${dim}`}
        role="img"
        aria-label={label}
        shapeRendering="crispEdges"
      >
        <rect width={dim} height={dim} fill="#ffffff" />
        <path d={d} fill="#000000" />
      </svg>
      {caption ? <figcaption className="muted">{caption}</figcaption> : null}
    </figure>
  );
}
