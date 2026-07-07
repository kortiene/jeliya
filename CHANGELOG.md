# Changelog

## [0.4.3] - 2026-07-07

### Changed

- Made file cards show honest fetch states: checking availability, ready to fetch, fetching, fetched, failed, and no provider online.
- Replaced fetched-file status-only labels with direct `Open file` and `Copy path` actions.
- Added a `Recheck` action for files whose providers are currently offline.

### Fixed

- Stopped showing `Fetch` for files that have already been fetched or have no online provider.
- Improved file-row layout so provider status and file actions stay readable on desktop and mobile.

## [0.4.2] - 2026-07-07

### Added

- Added a support diagnostics panel in Settings so users can copy a privacy-safe snapshot for bug reports.
- Added a GitHub bug report form with a dedicated field for pasted Jeliya diagnostics.

### Changed

- Captured the latest UI action error across room, message, file, pipe, join, create, and leave flows so reports include the failing context without exposing room contents.
