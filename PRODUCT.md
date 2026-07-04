# Product

## Register

product

## Users

Small teams of humans and their AI agents working together in private
peer-to-peer rooms. The humans are technical operators — they run daemons,
invite peers, watch agent fleets, open pipes to local ports. The agents are
full participants: they post statuses, artifacts, and results into the same
timeline. Primary context is a desktop working session (three-pane shell);
mobile is for checking in on rooms, agents, files, and pipes on the go
(bottom tab bar). The user is always mid-task: chatting, sharing a file,
reviewing an agent run, connecting a pipe.

## Product Purpose

Bantaba is the product shell over the Iroh Rooms P2P runtime — the gathering
place where a team's chat timeline, shared files, live pipes, and agent
statuses come together. Everything rendered is a fold over a signed event
log; no central server holds the rooms. Success looks like a room you can
trust: the state on screen is provably real, peers connect directly when
they can, and agents report what actually happened.

## Brand Personality

Calm, truthful, communal. *Bantaba* is the Mandinka word for the gathering
place under the village meeting tree — the interface is a quiet, dark room
where work gathers, not a dashboard shouting metrics. Terminal-adjacent
confidence (mono identifiers, dense panels, emerald signal) without terminal
coldness: rounded surfaces, human avatars, warm status language.

## Anti-references

- **Generic SaaS dashboard slop**: cream/light defaults, purple-blue
  gradients, identical KPI card grids, hero-metric tiles.
- **Fake-state UI**: optimistic "delivered" checks, spinners implying
  progress that isn't happening, silent partial file fetches. The runtime's
  honesty rules (README) forbid these outright.
- **Crypto/web3 aesthetic**: glow-everything, gradient text, neon hexagons —
  Bantaba is P2P but must not look like a token dashboard.
- **Chat-app skin**: this is not a Slack/Discord clone; agents, files, and
  pipes are first-class panes, not bolted-on integrations.

## Design Principles

1. **Truthful state over reassuring state.** Render what the log proves:
   direct/relay path badges, typed fetch failures (`unavailable` /
   `unauthorized` / `hash_mismatch`), real agent liveness. Never fake
   delivery, progress, or presence.
2. **The tool disappears into the task.** Earned familiarity: standard
   affordances, consistent component vocabulary, density where operators
   need it. No invented controls for standard jobs.
3. **One emerald voice.** The single accent (#2fd6a4) marks primary actions,
   current selection, and live/healthy state — never decoration.
4. **Calm ground, legible signal.** Dark teal-black surfaces stay quiet so
   the few status colors (emerald, amber, red, blue) read instantly.
5. **Every state is designed.** Agents, pipes, and files have many honest
   states (running, awaiting review, idle, unavailable, unauthorized).
   Each gets a deliberate presentation — including empty and failure states.

## Accessibility & Inclusion

WCAG 2.1 AA floor, already enforced in code: `--text-mute` is documented in
`ui/src/styles.css` as ≥4.5:1 on every surface because it colors small
information-bearing text. Keep that bar for any new text/surface pairing.
Status is never conveyed by color alone (dot + label). `prefers-reduced-motion`
must be honored. Full keyboard operability across composer, tabs, room list,
and modals.
