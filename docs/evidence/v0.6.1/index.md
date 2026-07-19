# v0.6.1 evidence boundary

This directory is reserved for fresh `v0.6.1` qualification evidence. It does
not inherit, copy, or modify the signed `v0.6.0` manifests.

The version-preparation pull request does not qualify a candidate. After that
pull request merges, maintainers must designate the exact public Jeliya commit
and the exact Iroh Rooms revision, create the dedicated qualification refs only
with explicit authorization, and run direct followed by forced-relay
qualification from that public source. Only the resulting sanitized
`direct.json`, `relay.json`, and their detached signatures belong here.

Until those four files are reviewed and committed in a documentation-only pull
request, the `v0.6.1` evidence gate remains blocked.
