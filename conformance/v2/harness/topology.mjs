// Actor topology for the v2 conformance harness (175c0b).
//
// The corpus names actors by label (`on: "subject:B"`); a daemon holds
// exactly ONE subject, so every distinct subject is a distinct daemon. This
// module is the ONE declared named-role → daemon registry the runner uses to
// resolve a label onto four finite axes:
//
//   {daemon_slot, subject_slot, principal_slot, connection_slot}
//
// There is no regex routing and no default-to-primary: a label that is not
// declared here fails the case BEFORE any step runs, and a label whose
// topology slot the case's `requires` never staged fails with a missing-role
// error naming both the label and the require that would stage it. The
// previous behavior — routing by /second|outsider|principal_b|peer|remote/i
// onto whichever daemon existed — silently ran invitee steps on the
// authority's daemon whenever the second daemon was missing (the false-green
// window the PR #311 review round refused).
//
// The catalog is FINITE and corpus-derived: topology.test.mjs enumerates
// every `on` label the corpus actually uses (step verbs, observe targets,
// control targets) and fails on any label this catalog does not declare, so
// the two can never drift. Where one label means different daemons in
// different domains, the override below says so explicitly, with the reason.

/**
 * The topology slots. A slot is a (daemon, subject) pair: one daemon, one
 * subject. Which slots a CASE stages is decided by its `requires` (see
 * planCaseTopology); the catalog only says where a label BELONGS.
 */
export const SLOTS = Object.freeze([
  'authority', // the case's primary daemon: subject $sa (aliases A/self)
  'invitee', // a redeeming joiner: subject $sb (second/invitee/joiner)
  'member', // an additional active member: subject $sd (B/member/member_b)
  'member2', // a second additional member (D/member_c; binds $member_c_sid)
  'outsider', // a subject with no room relationship: $sc (outsider/bystander/C)
  'agent', // an agent member (member:agent staging is not implemented)
]);

/** How a label participates on the principal axis. */
// canonical  — the slot's canonical (first) principal; #N-suffixed labels
//              SHARE it, so `subject:self#2` presents the same cid as
//              `subject:self` on a second socket.
// own        — a distinct principal named by this label (`subject:A2`,
//              `principal:self`, …): a second dedup principal on the same
//              daemon/subject. Never shared with another label.
// ephemeral  — no cid at all: the daemon's per-connection dedup principal
//              (an attached session).
// replace    — the slot's CANONICAL principal reconnecting: same cid, and
//              the new connection REPLACES the canonical connection.
export const PRINCIPAL_KINDS = Object.freeze(['canonical', 'own', 'ephemeral', 'replace']);

/**
 * The declared actor families. Every corpus label maps to exactly one entry;
 * entries name the slot, the principal kind, and (informationally) the
 * subject-id variable the staging binds for the slot.
 */
export const ACTOR_CATALOG = Object.freeze({
  // ── authority family: the primary daemon, subject $sa ──────────────────
  'subject:A': { slot: 'authority', principal: 'canonical' },
  'subject:authority': { slot: 'authority', principal: 'canonical' },
  'subject:self': { slot: 'authority', principal: 'canonical' },
  // Extra principals on the authority daemon (distinct cid, same subject).
  'subject:A2': { slot: 'authority', principal: 'own' },
  'principal:self': { slot: 'authority', principal: 'own' },
  'subject:principal_a': { slot: 'authority', principal: 'own' },
  'subject:principal_b': { slot: 'authority', principal: 'own' },
  'subject:principal_1': { slot: 'authority', principal: 'own' },
  'subject:principal_2': { slot: 'authority', principal: 'own' },
  'subject:authority_principal_1': { slot: 'authority', principal: 'own' },
  'subject:authority_principal_2': { slot: 'authority', principal: 'own' },
  // Same principal, reconnecting: replaces the canonical connection.
  'subject:same_principal': { slot: 'authority', principal: 'replace' },
  // ── invitee family: the redeeming joiner's daemon, subject $sb ─────────
  'subject:second': { slot: 'invitee', principal: 'canonical' },
  'subject:invitee': { slot: 'invitee', principal: 'canonical' },
  'subject:second_subject': { slot: 'invitee', principal: 'canonical' },
  'subject:joiner': { slot: 'invitee', principal: 'canonical' },
  'principal:second': { slot: 'invitee', principal: 'own' },
  // An attached session on the invitee slot: ephemeral per-connection
  // principal, own socket (`session:cX` in the push non-oracle case).
  'session:cX': { slot: 'invitee', principal: 'ephemeral' },
  // ── member family: an additional active member's daemon, subject $sd ───
  'subject:B': { slot: 'member', principal: 'canonical' },
  'subject:member': { slot: 'member', principal: 'canonical' },
  'subject:member_b': { slot: 'member', principal: 'canonical' },
  'subject:member_nonagent': { slot: 'member', principal: 'canonical' },
  // A departed member is still the member-family subject on its daemon.
  'subject:former_member': { slot: 'member', principal: 'canonical' },
  // ── second-member family: subject $se (member_c / D) ───────────────────
  'subject:D': { slot: 'member2', principal: 'canonical' },
  'subject:member_c': { slot: 'member2', principal: 'canonical' },
  // ── outsider family: no room relationship, subject $sc ─────────────────
  'subject:outsider': { slot: 'outsider', principal: 'canonical' },
  'subject:bystander': { slot: 'outsider', principal: 'canonical' },
  'subject:C': { slot: 'outsider', principal: 'canonical' },
  // ── agent family: staged by `member:agent` (not implemented) ───────────
  'subject:member_agent': { slot: 'agent', principal: 'canonical' },
});

/**
 * Per-domain overrides. A label whose slot differs in ONE domain says so
 * here, with the recorded reason; topology.test.mjs pins that the override
 * domains are exactly the domains where the label is used with that meaning
 * (and that the label is not used with its base-family meaning there).
 */
export const DOMAIN_OVERRIDES = Object.freeze({
  'subject-daemon.json': Object.freeze({
    // The one-past max-frame case proves the oversize frame was discarded
    // BEFORE dispatch by calling subject.ensure on a label that must hit the
    // SAME daemon the frame was sent to (`created:true` = the daemon never
    // executed an ensure). In this domain B names a second principal on the
    // primary daemon, not the pipes member peer.
    'subject:B': { slot: 'authority', principal: 'own' },
    // The subject-store fault case targets the case's one fresh daemon;
    // C observes that same daemon. (Its `daemon:restartable` require is
    // refused today, so the override is catalog completeness — recorded so
    // the routing is never implicit.)
    'subject:C': { slot: 'authority', principal: 'own' },
  }),
});

/**
 * Observe-only connection aliases. `c1` names the authority slot's first
 * EXTRA canonical-principal connection (the corpus always opens
 * `subject:self#2` before asserting on `c1`); topology.test.mjs pins the
 * co-occurrence so the alias can never dangle.
 */
export const OBSERVE_ALIASES = Object.freeze({
  c1: 'subject:self#2',
});

/** Labels that never route to a session (`on: "none"` probes unreachability). */
export const NON_SESSION_LABELS = Object.freeze(new Set(['none']));

/** A connection-suffix label: `subject:<base>#N` — N ≥ 2, base declared. */
const CONNECTION_SUFFIX = /^(.*)#([2-9][0-9]*)$/;

/** Thrown when a label does not resolve; fails the case BEFORE its steps. */
export class ActorResolutionError extends Error {
  constructor(message) {
    super(message);
    this.name = 'ActorResolutionError';
  }
}

/**
 * Resolve one actor label onto the four axes. `domain` is the corpus file
 * name (fixture `_file`); overrides apply per domain. Returns
 * `{slot, principalKind, principalKey, connectionKind, aliasOf?, baseLabel?}`:
 *  - principalKey identifies the principal-map entry ('canonical' for the
 *    slot's first principal, the label itself for own/ephemeral principals);
 *  - connectionKind 'canonical' | 'extra' | 'replace' — `#N` suffixes are
 *    EXTRA connections on the base label's principal (`#N` never changes the
 *    principal or the slot; `A2` is NOT a `#N` suffix — it is its own label).
 */
export function resolveActor(domain, label) {
  if (typeof label !== 'string' || label.length === 0) {
    throw new ActorResolutionError(`actor label ${JSON.stringify(label)} is not a string`);
  }
  if (NON_SESSION_LABELS.has(label)) {
    throw new ActorResolutionError(
      `actor label "${label}" is observe-only (daemon unreachability) and never routes a session`,
    );
  }
  const aliasOf = OBSERVE_ALIASES[label];
  if (aliasOf !== undefined) {
    const base = resolveActor(domain, aliasOf);
    return { ...base, aliasOf };
  }
  // `#N` affects the CONNECTION axis only (contract: `self#2`/`A#2` = same
  // cid, different socket). Parsed after the alias table so an alias can
  // point at a suffixed label.
  const suffixed = CONNECTION_SUFFIX.exec(label);
  if (suffixed) {
    const base = resolveActor(domain, suffixed[1]);
    return {
      ...base,
      baseLabel: suffixed[1],
      connectionKind: 'extra',
      label,
    };
  }
  const override = DOMAIN_OVERRIDES[domain]?.[label];
  const entry = override ?? ACTOR_CATALOG[label];
  if (!entry) {
    throw new ActorResolutionError(
      `actor label "${label}" is not declared in the harness actor catalog ` +
        `(topology.mjs) — undeclared labels fail rather than routing by guesswork`,
    );
  }
  return {
    slot: entry.slot,
    principalKind: entry.principal,
    principalKey: entry.principal === 'canonical' || entry.principal === 'replace'
      ? 'canonical'
      : label,
    // The CONNECTION axis: only a canonical-principal label shares the
    // slot's ONE canonical connection. An own-principal label (A2,
    // principal:self, principal:second, principal_1/2…) and an ephemeral
    // attached session (session:cX) each open their OWN connection —
    // sharing the canonical socket would collapse the principal axis
    // entirely (the reviewer-caught P1: the distinct cid was minted but
    // never presented, so two "principals" hit one dedup ledger and the
    // per-principal cases false-red with op_id_conflict).
    connectionKind: entry.principal === 'canonical' ? 'canonical'
      : entry.principal === 'replace' ? 'replace'
      : 'own',
    label,
  };
}

/** The labels whose sessions hold a slot's canonical connection. */
const CANONICAL_LABELS = Object.freeze([
  ['authority', 'subject:self'],
  ['invitee', 'subject:second'],
  ['member', 'subject:member_b'],
  ['member2', 'subject:member_c'],
  ['outsider', 'subject:outsider'],
  ['agent', 'subject:member_agent'],
]);

/**
 * Which slots a case's `requires` stage, and whether the daemons must run in
 * RealNetwork mode. PURE: no I/O, so the mode rules are unit-testable.
 *
 * RealNetwork (no `--loopback`) is required exactly when a daemon must reach
 * another daemon: `link:up` (the transport condition itself) or any member
 * join (`member:b`/`member:c`/`room:left` stage a second daemon that redeems
 * an invite, and the invited join dials the minter via discovery, which
 * loopback mode cannot do — see live_native_probe.rs). Everything else —
 * including `daemon:second`/`subject:second`, whose staging is a local
 * subject.ensure only — stays loopback.
 */
export function planCaseTopology(requires) {
  const req = new Set(requires || []);
  const slots = new Set(['authority']);
  if (req.has('daemon:second') || req.has('subject:second')) slots.add('invitee');
  if (req.has('subject:outsider')) slots.add('outsider');
  // `room:left` stages the member slot itself: a departed member is part of
  // its topology (create, invite, redeem, activate, leave), so the token is
  // self-sufficient — the staging joins and departs on the case's behalf.
  if (req.has('member:b') || req.has('room:left')) slots.add('member');
  if (req.has('member:c')) slots.add('member2');
  // `link:up` needs a second reachable daemon: when nothing else stages one,
  // the member slot is the link peer (the files-domain provider/consumer
  // shape). A case that already stages a second subject uses it as the peer.
  if (req.has('link:up')) {
    const hasPeer = slots.has('invitee') || slots.has('member') || slots.has('member2')
      || slots.has('outsider');
    if (!hasPeer) slots.add('member');
  }
  const realNetwork = req.has('link:up') || req.has('member:b') || req.has('member:c')
    || req.has('room:left');
  return Object.freeze({
    slots: Object.freeze([...slots]),
    realNetwork,
    linkUp: req.has('link:up'),
  });
}

/** Whether a case's daemons must run in RealNetwork mode (see above). */
export function caseNeedsRealNetwork(requires) {
  return planCaseTopology(requires).realNetwork;
}

/**
 * A bounded evidence poll: call `probe` until it returns `{done: true}` or
 * the deadline expires, then throw (a setup ERROR, never a pass). This is
 * the shape the harness's three existing deadline-poll precedents use
 * (portfile wait, process_exited, staging-residue settle); the link witness
 * and the RealNetwork canary share it so an unconverged topology can never
 * read as a green case.
 */
export async function pollUntil({ probe, deadlineMs, describe, intervalMs = 250 }) {
  const deadline = Date.now() + deadlineMs;
  let last = '(no probe result)';
  for (;;) {
    const r = await probe();
    last = r.detail ?? last;
    if (r.done) return r;
    if (Date.now() >= deadline) {
      throw new Error(
        `link topology witness did not converge within ${deadlineMs}ms: ${describe} (last: ${last})`,
      );
    }
    await new Promise((r2) => setTimeout(r2, intervalMs));
  }
}

let principalCounter = 1;

/**
 * The per-case topology state: staged daemons by slot, the principal map
 * (`slot|key` → cid), and which label currently holds each slot's canonical
 * connection (same_principal reconnects take it over). The runner owns
 * daemon processes; this class owns identity bookkeeping only.
 */
export class CaseTopology {
  constructor(domain) {
    this.domain = domain || '';
    this.daemons = new Map(); // slot → Daemon
    this.principalIds = new Map(); // `${slot}|${principalKey}` → cid
    this.canonicalLabel = new Map(CANONICAL_LABELS); // slot → label
  }

  /** Register the daemon for a slot. */
  setDaemon(slot, daemon) {
    this.daemons.set(slot, daemon);
  }

  /** The staged daemon for a slot, or null (missing-role detection). */
  daemonFor(slot) {
    return this.daemons.get(slot) ?? null;
  }

  /**
   * The cid a resolved actor presents. Ephemeral principals get none (the
   * daemon derives a per-connection principal). Canonical/own principals get
   * a stable id minted once per (slot, key) — reconnects and `#N` extra
   * connections reuse it, which is what makes `self#2` the same dedup
   * principal as `self` on a second socket.
   */
  principalIdFor(slot, principalKey, kind) {
    if (kind === 'ephemeral') return null;
    const key = `${slot}|${principalKey}`;
    let id = this.principalIds.get(key);
    if (id === undefined) {
      id = `cf-${slot}-${principalKey}-${principalCounter++}`;
      this.principalIds.set(key, id);
    }
    return id;
  }

  /**
   * The label a canonical-connection lookup should use: `same_principal`
   * reconnects REPLACE the canonical connection, so after such a step the
   * canonical label resolves to the replacement's live session.
   */
  canonicalLabelFor(slot) {
    return this.canonicalLabel.get(slot) ?? null;
  }

  /** Record that `label` now holds the slot's canonical connection. */
  setCanonicalLabel(slot, label) {
    this.canonicalLabel.set(slot, label);
  }
}
