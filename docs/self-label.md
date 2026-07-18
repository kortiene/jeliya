---
type: "Decision"
title: "Device-local self label — a friendly name for you, never signed"
description: "Decision record defining how Jeliya gives the local user an editable, device-local display label reusing the alias store keyed by the self identity id, its fallback, validation, migration, and privacy rules, and where self is rendered consistently across both clients."
tags: ["ux", "identity", "naming", "privacy"]
timestamp: "2026-07-18T12:00:00Z"
status: "canonical"
implementation_status: "planned"
verification_status: "unverified"
release_status: "unreleased"
audience: ["contributors", "maintainers", "product"]
---

# Device-local self label — a friendly name for you, never signed

**Status: DECIDED 2026-07-18.** On first run a user sees an opaque hex identity
id or a short id, with no clear "you". The identity cannot be renamed for local
display, and invite inputs used to begin with an unexplained long self id. This
record decides how the clients give the local user a friendly, editable name —
without ever pretending it is signed or shared profile data.

## Decision

The self label is **the local alias of the user's own identity id**. It reuses
the existing device-local alias store (React `jeliya.aliases.v1` in
localStorage; Flutter `aliases` in `app_prefs.json`), keyed by the self identity
id. There is no new store, no new key, and no wire field.

- **Resolution.** Displaying self resolves to `alias(selfId) ?? "You"`. Peers
  keep their existing order (`alias(id) ?? suggestion ?? shortId(id)`). The
  self fallback is the localized **"You"**, never the raw hex id.
- **Migration is automatic.** Because the label is just the self identity's
  alias, any alias a returning user already set for their own id becomes the
  label with no migration step. Users who never set one see "You" as before.
- **Local only, never signed.** The protocol has no display names. The label
  lives only on this device, is never sent, never appears in a signed event or
  roster, and is **excluded from diagnostics** (which already redact full
  identities). Every editor states this in copy.

## Validation (shared by both clients)

- Trim surrounding whitespace on save; internal spaces are preserved (names like
  "Alex K").
- An empty or whitespace-only value **clears** the label — self falls back to
  "You". This mirrors the existing "Clear alias" behaviour for peers.
- A soft maximum of 40 characters, enforced on the input.
- No format constraint beyond the above: it is a human display string, not an
  identifier.

## Rendering — self is identified consistently

Once self resolves to `alias(selfId) ?? "You"`, every self-rendering site routes
through the shared name resolver instead of hard-coding "You":

- **Sender name** and **avatar** in the timeline.
- The **profile card** (sidebar) and **settings** identity surfaces.
- The **roster** member row — which additionally keeps its distinct
  **"this device"** marker so "which one is me" never depends on the name.
- The **pipe** authorized-peer line.

The own-message side (right-aligned bubbles) and the "this device" chip are the
markers that identify *which* participant is the local user; the label is only
the friendly *name*. The two are orthogonal, so a user who names themselves
"Alex" is still unambiguously marked as this device.

## The identity id stays secondary

The cryptographic identity id remains visible but secondary: shortened by
default, fully copyable, and described as the unrecoverable P2P identity. The
friendly label never replaces it — onboarding, the sidebar profile handle, and
settings all keep the id reachable.

## Invitation inputs start empty

Invite identity inputs start **empty** with an example/help state, never
pre-seeded with the user's own long id (there is nothing to invite yourself to).
This was already the behaviour in both clients; it is locked here so it is not
regressed while the surrounding identity copy changes.

## First run and settings

- **First run** offers an optional device-label field alongside the created
  identity, so a new user can name themselves immediately.
- **Settings** exposes the same editor so an existing user can change the local
  label at any time, without touching the cryptographic identity.

## Cross-client parity

React and Flutter share the storage shape, the `alias(selfId) ?? "You"`
resolution, the trim/clear/max-length validation, the automatic migration, and
the privacy invariant. Flutter additionally localizes every string (EN + FR).

See [Room attention](room-attention.md) and [Room Workbench](room-workbench.md)
for the surrounding identity and status vocabulary.
