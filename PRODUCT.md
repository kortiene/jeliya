# Product

## Register

product

## Users

Small teams of humans and their AI agents working together in private
peer-to-peer rooms. The humans are technical operators — they run daemons,
invite peers, watch agent fleets, open pipes to local ports. The agents are
full participants: they post statuses, artifacts, and results into the same
timeline. Primary context is a desktop working session (the wide shell:
room rail, workspace, inspector); mobile is for checking in on rooms and
agent runs on the go, then stepping into a room's workbench. The user is
always mid-task: chatting, sharing a file, reviewing an agent run,
connecting a pipe.

Work is organized the same way on every screen: a few **global**
destinations (Rooms, Agent Fleet, Settings), and a **room's** workbench
(Activity, People, Agents & Runs, Files, Pipes) that exists once a room is
selected — files and pipes are always about one room, so they live in one.
The hierarchy, its routes, its responsive shells, and its status vocabulary
are recorded in `docs/room-workbench.md`.

## Product Purpose

Jeliya is the product shell over the Iroh Rooms P2P runtime — the gathering
place where a team's chat timeline, shared files, live pipes, and agent
statuses come together. Everything rendered is a fold over a signed event
log; no central server holds the rooms. Success looks like a room you can
trust: the state on screen is provably real, peers connect directly when
they can, and agents report what actually happened.

## Brand Personality

Calm, truthful, communal. *Jeliya* is the Manding word for the art of the
jeli — the hereditary keeper of the community's true record, who speaks
where the village gathers — and the interface holds that office: a quiet,
dark room where work gathers and nothing is quietly rewritten, not a
dashboard shouting metrics. Terminal-adjacent confidence (mono identifiers,
dense panels, emerald signal) without terminal coldness: rounded surfaces,
human avatars, warm status language.

## Anti-references

- **Generic SaaS dashboard slop**: cream/light defaults, purple-blue
  gradients, identical KPI card grids, hero-metric tiles.
- **Fake-state UI**: optimistic "delivered" checks, spinners implying
  progress that isn't happening, silent partial file fetches. The runtime's
  honesty rules (README) forbid these outright.
- **Crypto/web3 aesthetic**: glow-everything, gradient text, neon hexagons —
  Jeliya is P2P but must not look like a token dashboard.
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

WCAG 2.1 AA is the design target, not a current certification. The codebase
defines contrast intent for `--text-mute`, combines status color with labels,
and requires reduced-motion and keyboard behavior, but CI does not yet provide
a complete automated WCAG audit across every React and Flutter state. Treat
accessibility verification as partial until automated checks and documented
manual keyboard, screen-reader, contrast, zoom, and text-scaling evidence all
pass. See [`docs/capability-status.md`](docs/capability-status.md).
