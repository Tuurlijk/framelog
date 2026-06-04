#!/usr/bin/env bash
# Discover whether M241 / GPP0 / PMF-related ACPI symbols exist in decompiled tables.
set -euo pipefail

WORKDIR="${1:-/tmp/framelog-acpi}"
SEARCH="${WORKDIR}/dsl"

usage() {
  cat <<'USAGE'
Decompile ACPI tables (if needed) and search for M241 / GPP0 / PMF symbols.

Usage:
  scripts/discover-acpi-m241.sh [workdir]

Default workdir: /tmp/framelog-acpi

Prepare tables (once):
  sudo mkdir -p /tmp/framelog-acpi
  sudo cp /sys/firmware/acpi/tables/DSDT /tmp/framelog-acpi/
  sudo cp /sys/firmware/acpi/tables/SSDT* /tmp/framelog-acpi/ 2>/dev/null || true
  sudo chown -R "$USER:$USER" /tmp/framelog-acpi

Then run:
  scripts/discover-acpi-m241.sh
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if ! command -v iasl >/dev/null 2>&1; then
  echo "install acpica: sudo pacman -S acpica" >&2
  exit 1
fi

mkdir -p "$SEARCH"
shopt -s nullglob

tables=("$WORKDIR"/DSDT "$WORKDIR"/SSDT*)
if [[ ! -f "${tables[0]:-}" ]]; then
  echo "no DSDT in $WORKDIR — copy tables from /sys/firmware/acpi/tables first" >&2
  usage >&2
  exit 1
fi

echo "Decompiling ACPI tables into $SEARCH ..."
iasl -d -p "$SEARCH/" "${tables[@]}" 2>&1 | tail -5

dsl_files=("$SEARCH"/*.dsl)
if [[ ${#dsl_files[@]} -eq 0 ]]; then
  echo "no .dsl files produced" >&2
  exit 1
fi

echo ""
echo "=== M241 (exact) ==="
if grep -HnE 'Method \(M241|M241' "${dsl_files[@]}"; then
  :
else
  echo "(no matches)"
fi

echo ""
echo "=== GPP0 scope ==="
grep -Hn 'GPP0' "${dsl_files[@]}" | head -40 || echo "(no matches)"

echo ""
echo "=== PCI0 under \_SB_ (sample) ==="
grep -HnE 'Scope \(PCI0|Device \(GPP|GPP0' "${dsl_files[@]}" | head -40 || echo "(no matches)"

echo ""
echo "=== PMF / power-limit related (sample) ==="
grep -HniE 'PMF|STTC|STAPM|SPPT|FPPT|SPL|ryzen|amd_pmf' "${dsl_files[@]}" | head -40 || echo "(no matches)"

echo ""
echo "Done. If M241 is absent, the GitHub trigger path may not apply to this BIOS."
echo "See docs/framework-146-m241-trigger-capture.md"
