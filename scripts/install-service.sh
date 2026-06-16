#!/usr/bin/env bash
set -euo pipefail

PREFIX="/usr/local"
UNIT_DIR="/etc/systemd/system"
SERVICE_NAME="framelog.service"
DB_PATH="/var/lib/framelog/framelog.db"
BIND_ADDR="127.0.0.1:8787"
COLLECTOR="lib-amdgpu"
INTERVAL_MS="1000"
JOURNAL_BEFORE_SECS="60"
JOURNAL_AFTER_SECS="30"
RETENTION_HOURS="24"
BUILD="1"
ENABLE="1"
START="1"
APU_ONLY="0"

usage() {
  cat <<'USAGE'
Install framelog as a systemd service.

Usage:
  sudo scripts/install-service.sh [options]

Options:
  --prefix DIR             Install binary under DIR/bin (default: /usr/local)
  --unit-dir DIR           systemd unit directory (default: /etc/systemd/system)
  --db PATH                SQLite database path (default: /var/lib/framelog/framelog.db)
  --bind ADDR              Web UI bind address (default: 127.0.0.1:8787)
  --collector NAME         lib-amdgpu, intel, fake, or amdgpu-top-json (default: lib-amdgpu)
  --interval-ms N          Sample interval in milliseconds (default: 1000)
  --journal-before-secs N  Journal window before transitions (default: 60)
  --journal-after-secs N   Journal window after transitions (default: 30)
  --retention-hours N      Rolling SQLite retention in hours (default: 24, 0=keep all)
  --apu-only               Pass --apu-only to framelog run
  --no-build               Install existing target/release/framelog
  --no-enable              Do not enable the service at boot
  --no-start               Do not start/restart the service after install
  -h, --help               Show this help

Examples:
  sudo scripts/install-service.sh
  sudo scripts/install-service.sh --collector intel
  sudo scripts/install-service.sh --bind 0.0.0.0:8787 --no-start
USAGE
}

die() {
  echo "error: $*" >&2
  exit 1
}

systemd_quote_arg() {
  local arg="$1"
  arg="${arg//\\/\\\\}"
  arg="${arg//\"/\\\"}"
  arg="${arg//\$/\$\$}"
  arg="${arg//%/%%}"
  printf '"%s"' "$arg"
}

require_root() {
  if [[ "${EUID}" -ne 0 ]]; then
    die "run with sudo so the binary and systemd unit can be installed"
  fi
}

run_build() {
  if [[ -n "${SUDO_USER:-}" && "${SUDO_USER}" != "root" ]]; then
    local user_home
    local user_group
    user_home="$(getent passwd "${SUDO_USER}" | cut -d: -f6)"
    if [[ -z "$user_home" ]]; then
      die "could not determine home directory for ${SUDO_USER}"
    fi
    user_group="$(id -gn "${SUDO_USER}")"
    if [[ -d "${REPO_ROOT}/target" ]]; then
      chown -R "${SUDO_USER}:${user_group}" "${REPO_ROOT}/target"
    fi
    sudo -u "${SUDO_USER}" env HOME="${user_home}" PATH="${PATH}" \
      cargo build --manifest-path "${REPO_ROOT}/Cargo.toml" --release
  else
    cargo build --manifest-path "${REPO_ROOT}/Cargo.toml" --release
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      PREFIX="${2:?missing value for --prefix}"
      shift 2
      ;;
    --unit-dir)
      UNIT_DIR="${2:?missing value for --unit-dir}"
      shift 2
      ;;
    --db)
      DB_PATH="${2:?missing value for --db}"
      shift 2
      ;;
    --bind)
      BIND_ADDR="${2:?missing value for --bind}"
      shift 2
      ;;
    --collector)
      COLLECTOR="${2:?missing value for --collector}"
      shift 2
      ;;
    --interval-ms)
      INTERVAL_MS="${2:?missing value for --interval-ms}"
      shift 2
      ;;
    --journal-before-secs)
      JOURNAL_BEFORE_SECS="${2:?missing value for --journal-before-secs}"
      shift 2
      ;;
    --journal-after-secs)
      JOURNAL_AFTER_SECS="${2:?missing value for --journal-after-secs}"
      shift 2
      ;;
    --retention-hours)
      RETENTION_HOURS="${2:?missing value for --retention-hours}"
      shift 2
      ;;
    --apu-only)
      APU_ONLY="1"
      shift
      ;;
    --no-build)
      BUILD="0"
      shift
      ;;
    --no-enable)
      ENABLE="0"
      shift
      ;;
    --no-start)
      START="0"
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

case "$COLLECTOR" in
  lib-amdgpu|intel|fake|amdgpu-top-json) ;;
  *) die "--collector must be lib-amdgpu, intel, fake, or amdgpu-top-json" ;;
esac

[[ "$INTERVAL_MS" =~ ^[0-9]+$ ]] || die "--interval-ms must be an integer"
[[ "$JOURNAL_BEFORE_SECS" =~ ^[0-9]+$ ]] || die "--journal-before-secs must be an integer"
[[ "$JOURNAL_AFTER_SECS" =~ ^[0-9]+$ ]] || die "--journal-after-secs must be an integer"
[[ "$RETENTION_HOURS" =~ ^[0-9]+$ ]] || die "--retention-hours must be an integer"

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
BIN_DIR="${PREFIX}/bin"
BIN_PATH="${BIN_DIR}/framelog"
UNIT_PATH="${UNIT_DIR}/${SERVICE_NAME}"

require_root

if [[ "$BUILD" == "1" ]]; then
  echo "Building release binary..."
  run_build
fi

SOURCE_BIN="${REPO_ROOT}/target/release/framelog"
[[ -x "$SOURCE_BIN" ]] || die "release binary not found at ${SOURCE_BIN}; run cargo build --release or omit --no-build"

echo "Installing ${BIN_PATH}..."
install -D -m755 "$SOURCE_BIN" "$BIN_PATH"

run_args=(
  "$BIN_PATH"
  run
  --db "$DB_PATH"
  --bind "$BIND_ADDR"
  --collector "$COLLECTOR"
  --interval-ms "$INTERVAL_MS"
  --journal-before-secs "$JOURNAL_BEFORE_SECS"
  --journal-after-secs "$JOURNAL_AFTER_SECS"
  --retention-hours "$RETENTION_HOURS"
)
if [[ "$APU_ONLY" == "1" ]]; then
  run_args+=(--apu-only)
fi

exec_start=""
for arg in "${run_args[@]}"; do
  if [[ -z "$exec_start" ]]; then
    exec_start="$(systemd_quote_arg "$arg")"
  else
    exec_start+=" $(systemd_quote_arg "$arg")"
  fi
done

echo "Installing ${UNIT_PATH}..."
install -d -m755 "$UNIT_DIR"
cat > "$UNIT_PATH" <<UNIT
[Unit]
Description=Framework firmware throttle flag logger (framelog)
After=network.target

[Service]
Type=simple
ExecStart=${exec_start}
Restart=on-failure
RestartSec=5
# Read gpu_metrics and PMF debugfs from sysfs/debugfs; root is simplest on first install.
User=root
Group=root
StateDirectory=framelog
LogsDirectory=framelog

[Install]
WantedBy=multi-user.target
UNIT
chmod 0644 "$UNIT_PATH"

echo "Reloading systemd..."
systemctl daemon-reload

if [[ "$ENABLE" == "1" ]]; then
  echo "Enabling ${SERVICE_NAME}..."
  systemctl enable "$SERVICE_NAME"
fi

if [[ "$START" == "1" ]]; then
  echo "Starting ${SERVICE_NAME}..."
  systemctl restart "$SERVICE_NAME"
fi

echo ""
echo "Installed framelog service."
echo "Dashboard: http://${BIND_ADDR}"
echo "Status:    systemctl status ${SERVICE_NAME}"
echo "Logs:      journalctl -u ${SERVICE_NAME} -f"
