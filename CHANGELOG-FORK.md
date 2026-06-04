# Fork Changelog

This file tracks changes maintained on the `phromo-fork` branch that may diverge
from upstream `agavra/tuicr`. Keep entries focused on integration-relevant
behavior, touched areas, and migration concerns.

## Unreleased

### Startup Commands And Async EOF Context Loading

- Added `startup_commands` config support.
  - Commands run after startup config is applied.
  - Entries may include or omit the leading `:`.
  - Only view/config commands are allowed; lifecycle, export, mutation, submit,
    editor, reload, and selector commands remain interactive-only.
  - Example: `startup_commands = ["set nocommits"]`.
- Added validation and warnings for invalid `startup_commands` config values.
- Added safe startup command dispatch through `handler::run_startup_command`.
- Moved EOF context line counting to a background loader.
  - Avoids eager file reads / VCS snapshots during startup.
  - Shows loading progress in the status bar.
  - Shows a dim ellipsis marker beside files whose EOF line count is still
    loading.
  - EOF context expansion warns if the line count is still loading.

Integration notes:
- Touched `src/config/mod.rs`, `src/handler.rs`, `src/main.rs`,
  `src/app.rs`, `src/ui/file_list.rs`, and `src/ui/status_bar.rs`.
- Watch for upstream changes around config parsing, command dispatch, diff gap
  expansion, and status/file-list rendering.

### Markdown-Backed Review Files

- Added explicit `--review-file <path.md>` support for TUI invocations.
  - Without this flag, tuicr continues to use the built-in per-user review
    cache.
  - With this flag, the selected markdown file is the persistence target.
- Added `--review-file <path.md>` support to `tuicr review list/add/comments`.
- Added `src/persistence/markdown.rs`.
  - The file body uses human-readable, revdiff-like comment headings such as
    `## src/main.rs:42 (ISSUE, new)`.
  - Lossless tuicr state is stored in YAML frontmatter.
  - Frontmatter includes `tuicr_session_json: |-` with compact pretty JSON.
  - Nested JSON containers stay on one line when they fit within 100 columns.
  - The loader remains backward-compatible with the first MVP hidden HTML
    comment state block.
- Routed `ReviewStore` through markdown persistence when constructed with a
  review file.
- Routed TUI save/reload/external-merge paths through markdown persistence when
  `--review-file` is active.
- Existing markdown review files are adopted to the live invocation identity.
  The live target overwrites identity fields such as repo path, base commit,
  diff source, commit range, and PR session key; review comments/state from the
  markdown file are retained and re-registered against the current diff.

Integration notes:
- Touched `src/cli.rs`, `src/main.rs`, `src/app.rs`,
  `src/persistence/mod.rs`, `src/persistence/markdown.rs`,
  `src/review_cli.rs`, `src/review_store.rs`, `README.md`, and
  `docs/REVIEW_CLI.md`.
- Watch for upstream changes around `ReviewStore`, session storage layout,
  active-session tracking, CLI argument parsing, and PR/session identity.
- Potential future hardening: detect divergent existing review files and require
  an explicit adopt/force flag before rewriting their identity fields.
