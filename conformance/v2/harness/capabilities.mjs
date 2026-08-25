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
 * vocabulary — link:*, room:removed, room:foreign, member:agent,
 * member:non_agent, daemon:restartable, observe:*, resource:large_file,
 * resource:fetched_file, fault:*, control:concurrency — is NOT established,
 * and the runner throws before staging anything when a case declares it.
 *
 * Deliberately NOT listed (staged unfaithfully — refusing loudly is the
 * honest state until their setup is real; PR #311 review round):
 *  - `room:quiescent` / `room:with_history`: #establishRequires stages both
 *    exactly like `room:live` (activate; no history is ever authored), so
 *    claiming either state would be a claim the staging cannot back.
 *  - `room:left`: `$rid_left` is created but no member ever joins and
 *    leaves it, so it is not a left room.
 *  - `member:b` / `member:c`: their sessions route to the PRIMARY daemon
 *    (the routing regex matches second/outsider/principal_b/peer/remote
 *    only), whose single subject is the authority's — the "member" is not
 *    a distinct invited subject.
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
