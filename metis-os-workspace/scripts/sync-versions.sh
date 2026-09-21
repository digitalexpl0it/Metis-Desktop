#!/usr/bin/env bash
# Alias for sync-version.sh (singular). Kept so docs / muscle memory that say
# "sync-versions" still work.
exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/sync-version.sh" "$@"
