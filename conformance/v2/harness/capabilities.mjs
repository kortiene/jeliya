// The ONE capability classification for the protocol-v2 conformance harness.
//
// The checker (scripts/check-v2-corpus.mjs), the runner (runner.mjs), the
// corpus README's control table, and the harness tests all import THIS module,
// so the executable / documented-only lists cannot drift between them:
//  - the checker asserts its closed `control.do` vocabulary equals the keys of
//    CONTROL_CAPABILITIES, and that every REQUIRES_IMPLEMENTED token is a
//    well-formed member of its own `requires` vocabulary;
//  - the runner refuses (before staging anything) a `requires` token that is
//    not in REQUIRES_IMPLEMENTED, and refuses a documented-only control verb
//    at execution time — never silently ignoring either;
//  - harness.test.mjs asserts the README control table's Harness column and
//    this module agree row by row.
//
// `documented_only` is a promise, not an excuse: the DSL shape-checks the verb
// (a malformed documented-only control fails the checker exactly like a
// malformed executable one), and the runner fails loudly the moment a replay
// reaches it. Promoting a verb to `executable` is a harness change that must
// update the README table and this module in the same commit.

/**
 * Control verbs: executable by this harness, or documented in the DSL but not
 * yet executed. `advance_clock` is executable with a documented degradation —
 * the harness cannot move the daemon's clock, so it performs a real wait
 * capped at one second (runner.mjs #doControl).
 */
export const CONTROL_CAPABILITIES = Object.freeze({
  advance_clock: 'executable',
  idle: 'executable',
  disconnect: 'executable',
  reconnect: 'executable',
  stop_daemon: 'executable',
  start_daemon: 'documented_only',
  set_limit: 'documented_only',
  set_link_rate: 'documented_only',
  start_transfers: 'documented_only',
  pause_link: 'documented_only',
  inject_fault: 'documented_only',
  set_provider_response_bytes: 'documented_only',
  client_preflight: 'documented_only',
  client_render_limit: 'documented_only',
  client_render_file: 'documented_only',
  cancel_transfers: 'documented_only',
});

/** Every control verb this harness can execute. */
export const EXECUTABLE_CONTROLS = new Set(
  Object.entries(CONTROL_CAPABILITIES).filter(([, c]) => c === 'executable').map(([do_]) => do_),
);

/**
 * `requires` tokens this runner genuinely establishes today.
 * Every OTHER well-formed token in the checker's closed `requires`
 * vocabulary — link:down, link:relay, link:slow, room:removed,
 * room:foreign, room:quiescent, room:with_history, member:agent,
 * member:non_agent, daemon:restartable, observe:*, resource:large_file,
 * resource:fetched_file, fault:*, control:concurrency — is NOT established,
 * and the runner throws before staging anything when a case declares it.
 *
 * 175c0b promoted four tokens from the PR #311 refusal list, each backed by
 * real staging (topology.mjs):
 *  - `link:up`: the case's daemons run WITHOUT `--loopback` (RealNetwork:
 *    the invited join dials the minter via discovery, which loopback mode
 *    cannot do); a link peer is staged when the case stages no second
 *    subject; a per-case witness re-verifies the topology before steps, and
 *    main.mjs runs the RealNetwork canary before any selected link:up case.
 *  - `member:b` / `member:c`: a REAL additional member on its OWN daemon
 *    (one daemon, one subject): subject.ensure, invite.mint by the
 *    authority, invite.redeem over the RealNetwork link, room.activate.
 *  - `room:left`: a room a member genuinely joined and left (create, mint,
 *    redeem, activate, room.leave) — binding `$rid_left`.
 *
 * Deliberately NOT listed (refused loudly — the honest state until their
 * setup is real):
 *  - `room:quiescent` / `room:with_history`: faithful staging needs authored
 *    room history and liveness timelines (a deactivated or never-activated
 *    room with real events) — room-domain scenario work, not topology
 *    (175c2). Staging them exactly like `room:live` (the pre-175c0b shape)
 *    would claim states the setup cannot back.
 *  - `member:agent` / `member:non_agent`: agent/non-agent membership
 *    standing is invite-domain scenario staging (role/standing semantics),
 *    not topology resolution.
 *
 * `control:limits` is listed because the runner surfaces the daemon's served
 * limits to every case (the hello capture and the pre-seeded `$limits`); the
 * override verb `set_limit` itself stays documented-only and fails loudly if
 * a step actually uses it. `control:clock` is listed because `advance_clock`
 * executes (as the documented capped real wait).
 */
export const REQUIRES_IMPLEMENTED = Object.freeze(new Set([
  'subject',
  'subject:none',
  'subject:second',
  'subject:outsider',
  'daemon',
  'daemon:self',
  'daemon:fresh',
  'daemon:second',
  'room:plain',
  'room:live',
  'room:left',
  'member:b',
  'member:c',
  'link:up',
  'resource:tcp_service',
  'resource:shared_file',
  'control:reconnect',
  'control:limits',
  'control:clock',
]));

/**
 * Validate a case's `requires` list against what this runner establishes.
 * Returns null when every token is implemented, or an Error describing the
 * first unimplemented token. Pure: performs no I/O, so the runner calls it
 * before spawning any daemon.
 */
export function unimplementedRequire(requires) {
  for (const token of requires || []) {
    if (typeof token !== 'string') continue; // the checker owns shape errors
    if (REQUIRES_IMPLEMENTED.has(token)) continue;
    return new Error(
      `requires token "${token}" is well-formed corpus vocabulary but this harness does not ` +
        `establish it — refusing to run the case silently half-staged ` +
        `(implemented tokens: ${[...REQUIRES_IMPLEMENTED].sort().join(', ')})`,
    );
  }
  return null;
}
