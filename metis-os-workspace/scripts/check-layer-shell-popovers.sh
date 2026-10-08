#!/usr/bin/env bash
# Ban GTK Popover autohide in metis-shell layer-shell UI.
#
# Metis ignores xdg_popup grabs and bar/desktop surfaces use KeyboardMode::None,
# so autohide(true) popovers never present. Use autohide(false) + register +
# idle popup (see metis-shell/src/ui/bar/dropdown.rs).
#
# Scope: metis-shell only. Normal xdg windows (e.g. metis-settings) may use
# autohide(true).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SHELL_SRC="${ROOT}/metis-shell/src"

if ! [[ -d "${SHELL_SRC}" ]]; then
  echo "error: missing ${SHELL_SRC}" >&2
  exit 1
fi

PATTERN='autohide\(true\)|set_autohide\(true\)'
hits=""
if command -v rg >/dev/null 2>&1; then
  hits="$(rg -n --glob '*.rs' -e "${PATTERN}" "${SHELL_SRC}" || true)"
else
  hits="$(grep -RInE --include='*.rs' "${PATTERN}" "${SHELL_SRC}" || true)"
fi

if [[ -n "${hits}" ]]; then
  echo "error: metis-shell must not use Popover autohide(true) on layer-shell UI:" >&2
  echo "${hits}" >&2
  echo >&2
  echo "Use autohide(false) + crate::ui::bar::register_bar_popover + idle popup." >&2
  exit 1
fi

echo "ok: no autohide(true) in metis-shell"
