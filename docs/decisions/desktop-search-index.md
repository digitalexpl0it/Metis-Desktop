# Desktop search index (`metis-indexd`)

## Status

Accepted (Phase 26).

## Context

Metis needs content-aware desktop search (path + body) with first-class tags and
category facets, without shipping a file manager in this phase. The shell must
soft-fail when the indexer is absent.

## Decision

1. **Engine:** Tantivy only (no SQLite FTS dual path). Schema version `v1` lives
   under `$XDG_CACHE_HOME/metis/index/v1/`. Bumping the schema version uses a new
   directory so old indexes are abandoned cleanly.
2. **Process:** Out-of-process `metis-indexd`, session-spawned like
   `metis-secretsd`, listening on `$XDG_RUNTIME_DIR/metis/indexd.sock`
   (same-euid, mode `0600`). Session start must treat a leftover socket as
   stale unless a live `metis-indexd` process exists — `XDG_RUNTIME_DIR`
   often survives logout while the indexer does not.
3. **IPC:** Length-prefixed JSON (`metis-protocol::search`) —
   `Query` / `TagAdd` / `TagRemove` / `TagsList` / `SuggestTags` /
   `Status` / `ReloadConfig` / `Pause` / `Resume` / `Reindex`.
4. **User tags:** SQLite sidecar at `$XDG_DATA_HOME/metis/search-tags.sqlite`,
   keyed by path with `dev`+`ino` for rename resilience. Auto-tags
   (`cat:…`, `dir:…`, `year:YYYY`) are merged at index time; IPC tag mutate
   reindexes the path without requiring a full crawl. Shell preview edits
   user tags; Settings → File search browses tags and can remove them from
   a path.
5. **Config:** `~/.config/metis/search.json` (roots, excludes, pause, size caps,
   extractor toggles). Defaults index `$HOME` with common excludes
   (`.git`, `node_modules`, `target`, caches, Trash).
6. **Extractors:** UTF-8 / text / common source (default on). HTML strip
   (tags/scripts/styles → visible text) and PDF via optional `pdftotext`
   (poppler-utils; soft-fail if missing) are off by default until enabled
   in Settings. `ReloadConfig` rebuilds the extractor set without a full
   crawl; reindex to refresh already-indexed bodies.
7. **UI:** Edge-bar Files search overlay opens/reveals via
   `resolve_file_manager()`; Metis FM remains deferred. Filter chips are
   Documents / Images / Video / Audio / Archives (source/text under Documents).
8. **Rich preview (shell-side, tool-optional):** PDF first-page image via
   `pdftoppm` when present; Office text Contents via LibreOffice
   (`soffice`/`libreoffice`) when present. Missing tools soft-fail to the
   type icon / no Contents pane (same as plain documents without extractable
   text). Preview tooling is separate from index extractors and must not
   block the GTK UI thread.

## Consequences

- Shell/Settings depend only on the socket contract; missing daemon returns soft
  errors.
- Schema changes require a new cache directory (`v2`, …) and a one-shot reindex.
- Large `$HOME` trees need idle-friendly crawl + excludes; pause is first-class.
- Document preview quality depends on optional host packages (poppler-utils,
  LibreOffice); Metis must not hard-Depend on LibreOffice.
