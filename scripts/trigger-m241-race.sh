#!/usr/bin/env bash
set -euo pipefail

METHOD='\_SB_.PCI0.GPP0.M241'
DELAY_SECONDS="0.1"
ITERATIONS="1"
COOLDOWN_SECONDS="2"
DRY_RUN="0"
CONFIRMED="0"

usage() {
  cat <<'USAGE'
Trigger the Framework #146 M241 race reproduction sequence.

WARNING: This writes to /proc/acpi/call. It is intended only for Framework
AMD power-limit debugging when you understand the risk and are already
collecting evidence with framelog.

Usage:
  sudo scripts/trigger-m241-race.sh --yes-i-understand [options]

Options:
  --iterations N        Number of trigger attempts (default: 1)
  --delay SECONDS      Delay between M241(1) and M241(0) (default: 0.1)
  --cooldown SECONDS   Delay after each attempt (default: 2)
  --method METHOD      ACPI method path (default: \_SB_.PCI0.GPP0.M241)
  --dry-run            Print commands without writing to /proc/acpi/call
  --yes-i-understand   Required acknowledgement for non-dry-run execution
  -h, --help           Show this help

Example:
  sudo scripts/trigger-m241-race.sh --yes-i-understand --iterations 10 --delay 0.1
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --iterations)
      ITERATIONS="${2:?missing value for --iterations}"
      shift 2
      ;;
    --delay)
      DELAY_SECONDS="${2:?missing value for --delay}"
      shift 2
      ;;
    --cooldown)
      COOLDOWN_SECONDS="${2:?missing value for --cooldown}"
      shift 2
      ;;
    --method)
      METHOD="${2:?missing value for --method}"
      shift 2
      ;;
    --dry-run)
      DRY_RUN="1"
      shift
      ;;
    --yes-i-understand)
      CONFIRMED="1"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if ! [[ "$ITERATIONS" =~ ^[0-9]+$ ]] || [[ "$ITERATIONS" -lt 1 ]]; then
  echo "--iterations must be a positive integer" >&2
  exit 2
fi

if [[ "$DRY_RUN" != "1" && "$CONFIRMED" != "1" ]]; then
  echo "refusing to write ACPI calls without --yes-i-understand" >&2
  usage >&2
  exit 2
fi

if [[ "$DRY_RUN" != "1" ]]; then
  if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
    echo "must run as root because /proc/acpi/call is root-writable" >&2
    exit 1
  fi
  if [[ ! -w /proc/acpi/call ]]; then
    echo "/proc/acpi/call is not writable; load the acpi_call module first" >&2
    echo "try: sudo modprobe acpi_call" >&2
    exit 1
  fi
fi

log_marker() {
  local message="$1"
  printf '%s %s\n' "$(date --iso-8601=ns)" "$message"
  if command -v systemd-cat >/dev/null 2>&1; then
    printf '%s\n' "$message" | systemd-cat -t framelog-m241-trigger -p info || true
  elif command -v logger >/dev/null 2>&1; then
    logger -t framelog-m241-trigger "$message" || true
  fi
}

call_acpi() {
  local arg="$1"
  local command="${METHOD} ${arg}"
  if [[ "$DRY_RUN" == "1" ]]; then
    log_marker "dry-run: echo '${command}' >/proc/acpi/call"
    return
  fi

  printf '%s\n' "$command" >/proc/acpi/call
  local result
  result="$(tr -d '\000' </proc/acpi/call 2>&1 || true)"
  log_marker "called ${command}; result=${result}"
  if [[ "$result" == *"AE_NOT_FOUND"* ]]; then
    echo "ACPI method not found: ${METHOD}" >&2
    echo "Find the platform's actual M241 path before retrying." >&2
    exit 3
  fi
}

log_marker "starting M241 race trigger: iterations=${ITERATIONS} delay=${DELAY_SECONDS}s cooldown=${COOLDOWN_SECONDS}s method=${METHOD}"

for ((i = 1; i <= ITERATIONS; i++)); do
  log_marker "attempt ${i}/${ITERATIONS}: assert M241(1)"
  call_acpi "0x01"
  sleep "$DELAY_SECONDS"
  log_marker "attempt ${i}/${ITERATIONS}: clear M241(0)"
  call_acpi "0x00"
  if [[ "$i" -lt "$ITERATIONS" ]]; then
    sleep "$COOLDOWN_SECONDS"
  fi
done

log_marker "completed M241 race trigger"
