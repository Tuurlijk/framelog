# framelog

Local telemetry for **AMDGPU independent throttle flags**, correlated with **systemd journal** entries and machine context (power, PMF, dGPU runtime). Built to produce shareable evidence for [Framework Laptop 16 power-limit debugging](https://github.com/FrameworkComputer/SoftwareFirmwareIssueTracker/issues/146).

## Why this exists

[Framework issue #146](https://github.com/FrameworkComputer/SoftwareFirmwareIssueTracker/issues/146) tracks cases where the CPU stays capped (for example around 35W) while the dGPU is asleep, with throttle flags such as `SPL` or `PROCHOT_CPU` still asserted. Reproducing that needs a timeline that ties together:

- Which throttle flags were active, and when they changed
- APU power and temperature samples
- Charger / power-profile / sleep context
- AMD PMF limits from Linux (`pmf.*`) when debugfs is readable
- dGPU DRM runtime state (`gpu_power.*`)
- Nearby journal messages around each transition

`framelog` runs continuously on the laptop, stores everything in SQLite, and exposes a local web UI plus JSON exports so you (or Framework engineers) can inspect a session without re-running the repro on the same machine.

Collection is **read-only**: it does not change firmware, ACPI tables, or PMF settings.

See [docs/prochot-status-investigation.md](docs/prochot-status-investigation.md) if **PROCHOT_CPU** / **PROCHOT_GPU** show 100% active on Radeon 780M — the raw metrics are often correct, but the meaning can be misleading on legacy `gpu_metrics` v2.1.

> **Contribute:** Pull requests are welcome. Please report bugs, request features,
> or share capture feedback in the
> [framelog issue tracker](https://github.com/Tuurlijk/framelog/issues).

## Features

- Samples `gpu_metrics` via [`libamdgpu_top`](https://crates.io/crates/libamdgpu_top) (no `amdgpu_top` binary required)
- Stores time series in **SQLite**
- Detects **flag transitions** (0→1 and 1→0)
- On each transition, queries journal logs in a configurable window (default 60s before, 30s after)
- Local **web UI** with uPlot graphs at `http://127.0.0.1:8787`
- **Modular context sources** (power, battery, display, profile, sleep, PMF, dGPU power)
- Cached **system inventory** (CPU, GPU, memory, storage, BIOS, Linux) in UI and exports
- **Guided capture** CLI (`framelog capture`) for clean, attachable evidence folders
- **Evidence Doctor** — deterministic analysis in the UI (`/analyze`), API, and `framelog analyze`

## Framework laptop compatibility

| Platform | Collector | Throttle flags | Package power | PMF / #146 rules |
|----------|-----------|----------------|---------------|------------------|
| Framework 13/16 **AMD** (Radeon iGPU/dGPU) | `lib-amdgpu` | Yes | Yes (APU package power) | Yes when debugfs PMF is readable |
| Framework 13 **Intel** | `intel` | No | Yes (RAPL/hwmon) | Context only; AMD rules skipped |
| Other AMDGPU laptops | `lib-amdgpu` | Usually | Usually | PMF only on AMD PMF firmware |

Check fit before a long capture:

```bash
framelog capabilities
framelog inspect --collector lib-amdgpu   # AMD
framelog inspect --collector intel        # Intel-only
```

The **System** page (`/system`) and `GET /api/capabilities` show detected DRM cards, recommended collector, and which Evidence Doctor rules apply.

## Requirements

- Linux with AMDGPU (`amdgpu`) for `lib-amdgpu` / default collector
- Read access to `/sys/class/drm/card*/device/gpu_metrics` (often requires root)
- Optional: systemd journal for transition correlation
- Optional: `/sys/kernel/debug/amd_pmf/current_power_limits` for PMF context (may require root)

## Build

```bash
cargo build --release
sudo install -m755 target/release/framelog /usr/local/bin/framelog
```

## Install Service

Install the release binary and register the systemd service:

```bash
sudo scripts/install-service.sh
```

Useful variants:

```bash
sudo scripts/install-service.sh --collector intel
sudo scripts/install-service.sh --bind 0.0.0.0:8787 --no-start
cargo build --release
sudo scripts/install-service.sh --no-build
```

The installer writes `/usr/local/bin/framelog`, installs `framelog.service`,
runs `systemctl daemon-reload`, then enables and starts the service by default.
After install:

```bash
systemctl status framelog.service
journalctl -u framelog.service -f
xdg-open http://127.0.0.1:8787
```

## Quick start

**Continuous logging (typical on a Framework 16):**

```bash
sudo scripts/install-service.sh

# open dashboard — live DB updates while the service runs
xdg-open http://127.0.0.1:8787
```

**One-shot evidence session for a bug report:**

```bash
sudo systemctl stop framelog.service
framelog capture --duration 20m --issue framework-146
framelog serve
# dashboard → Load export → pick export.json from the capture folder
```

See [in-app help](http://127.0.0.1:8787/help) for live vs export playback and the full capture workflow.

## Commands

| Command | Purpose |
|---------|---------|
| `framelog run` | Collector + web UI (same as systemd service) |
| `framelog collect` | Collector only, no web server |
| `framelog serve` | Web UI only; reads existing SQLite DB |
| `framelog inspect` | One live sample to stdout (debug, not the web UI) |
| `framelog capture` | Guided session: backup, reset, collect, export folder |
| `framelog dump` | Write timestamped JSON export from the DB |
| `framelog analyze` | Deterministic findings from an `export.json` (Evidence Doctor) |
| `framelog reset --yes` | Clear telemetry (optional backup first) |

Global flags (all subcommands): `--db`, `--interval-ms`, `--bind`, `--journal-before-secs`, `--journal-after-secs`, `--apu-only`, `--collector` (`lib-amdgpu`, `fake`, `amdgpu-top-json`).

```bash
framelog run --db ~/.local/share/framelog/framelog.db
framelog serve
framelog inspect
framelog capture --duration 20m --issue framework-146
framelog dump --output-dir ~/exports
framelog analyze export.json
framelog analyze export.json --format json --ai-prompt
framelog reset --yes
framelog run --collector fake   # no GPU required
```

### Options

| Flag | Default | Description |
|------|---------|-------------|
| `--db` | `~/.local/share/framelog/framelog.db` | SQLite path (`FRAMELOG_DB` env) |
| `--interval-ms` | `1000` | Sample interval |
| `--bind` | `127.0.0.1:8787` | Web listen address |
| `--journal-before-secs` | `60` | Journal window before transitions |
| `--journal-after-secs` | `30` | Journal window after transitions |
| `--apu-only` | off | Only sample APU-like devices |
| `--collector` | `lib-amdgpu` | `lib-amdgpu`, `fake`, `amdgpu-top-json`, or `intel` |

`capture` also accepts `--duration` (required), `--issue` (default `capture`), `--output-dir`, `--device-pci`, `--skip-pre-backup`.

`dump` accepts `--output`, `--output-dir`, `--from-ms`, `--to-ms`, `--device-pci`. CLI dumps include an `analysis` block (same rules as Evidence Doctor).

`analyze` accepts `--format markdown|json`, `--output`, `--ai-prompt`, `--from-ms`, `--to-ms`, `--device-pci`.

`reset` requires `--yes`. It writes a backup export first unless `--no-backup` is set; use `--backup` or `--backup-dir` to control the path.

## Using the web UI

| URL | Purpose |
|-----|---------|
| [http://127.0.0.1:8787/](http://127.0.0.1:8787/) | Dashboard: charts, timeline, export, load export |
| [http://127.0.0.1:8787/analyze](http://127.0.0.1:8787/analyze) | Evidence Doctor: findings for the selected time range |
| [http://127.0.0.1:8787/help](http://127.0.0.1:8787/help) | Workflows: capture, live vs playback |
| [http://127.0.0.1:8787/system](http://127.0.0.1:8787/system) | CPU/GPU inventory and issue #146 notes |

**Live inspection** — While `framelog run` or the systemd service is collecting, open the dashboard. It reads the growing database (use **Tail (live)** and **Refresh**). Logging continues in the background.

**Export playback** — Run `framelog serve` without a collector. Click **Load export** and choose `export.json`. Data stays in the browser; nothing is written to SQLite. Use **Return to live** to switch back to the database. There is no separate playback-only daemon.

**Share a window** — On the dashboard, set a time range and click **Export window** (JSON bundle). Recipients use **Load export** in their own UI.

**Analyze a range** — Set the dashboard time window and click **Analyze range**, or open the [Analysis](http://127.0.0.1:8787/analyze) page after loading an export. Live DB mode uses `GET /api/findings`; loaded exports use `POST /api/analyze/export` in the browser only. Findings are deterministic rules (Framework #146 shape, thermal vs power cap, missing PMF, and similar), not cloud AI. Use **Copy AI prompt** on the analysis page if you want an external model to explain the report further.

**Export window** — Dashboard **Export window** is capped at **7 days** and **500k samples** (same limits as `GET /api/export`). Use `framelog dump` or `capture` for full-range bundles.

The UI includes flag presets (power / thermal / current), power/temperature overlays, journal snippets on transitions, and event timeline playback (**Play** / **Prev** / **Next**).

## Guided capture

`framelog capture` produces a folder ready to attach to [issue #146](https://github.com/FrameworkComputer/SoftwareFirmwareIssueTracker/issues/146) or similar reports:

1. Stop any collector using the same DB: `sudo systemctl stop framelog.service`
2. Run capture while reproducing the issue: `framelog capture --duration 20m --issue framework-146`
3. Inspect offline: `framelog serve` → **Load export** → `export.json`
4. Attach `export.json`, `report.md`, and `summary.txt` to the GitHub issue
5. Restart logging when done: `sudo systemctl start framelog.service`

```text
framelog-capture-framework-146-<timestamp>/
  export.json
  analysis.json
  manifest.json
  system.json
  report.md
  summary.txt
  pre_capture_backup.json   # only if DB had older data
```

```bash
framelog capture --duration 1h --issue framework-146 --output-dir ~/captures/run-1
framelog capture --duration 300s --collector fake --skip-pre-backup
```

## Data maintenance

`framelog dump` exports the full stored range (CLI has no 7-day web limit). `dump` and `reset` backups embed an `analysis` block in the JSON.

`framelog capture` and `framelog reset --yes` refuse to run while the database is still receiving samples (for example when `framelog.service` is active). Stop the collector first:

```bash
sudo systemctl stop framelog.service
```

If a collector was just stopped, wait until no new samples arrive for about five seconds, or the command exits with `database still receiving samples`.

After a successful `reset`, start the service again if you want continuous logging:

```bash
framelog dump --output-dir ~/framelog-exports
framelog reset --yes --backup-dir ~/framelog-exports
sudo systemctl start framelog.service
```

## systemd

The repository includes an example service unit at `systemd/framelog.service`.
It runs `framelog run` as root because the first install path usually needs
read access to AMDGPU `gpu_metrics`, debugfs PMF limits, and the journal.

Build and install the binary:

```bash
cargo build --release
sudo install -m755 target/release/framelog /usr/local/bin/framelog
```

Install the included unit:

```bash
sudo install -D -m644 systemd/framelog.service /etc/systemd/system/framelog.service
sudo systemctl daemon-reload
sudo systemctl enable --now framelog.service
```

Or create/edit it manually:

```bash
sudoedit /etc/systemd/system/framelog.service
```

```ini
[Unit]
Description=Framework firmware throttle flag logger (framelog)
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/framelog run --db /var/lib/framelog/framelog.db --bind 127.0.0.1:8787 --journal-before-secs 60 --journal-after-secs 30
Restart=on-failure
RestartSec=5
# Root keeps first install simple for gpu_metrics, debugfs PMF, and journal access.
User=root
Group=root
StateDirectory=framelog
LogsDirectory=framelog

[Install]
WantedBy=multi-user.target
```

Validate and operate the service:

```bash
sudo systemd-analyze verify /etc/systemd/system/framelog.service
sudo systemctl daemon-reload
sudo systemctl restart framelog.service
systemctl status framelog.service
journalctl -u framelog.service -f
```

The unit stores data in `/var/lib/framelog/framelog.db` and serves the UI on
`http://127.0.0.1:8787`. For a clean guided capture, stop the service first:

```bash
sudo systemctl stop framelog.service
framelog capture --duration 20m --issue framework-146
sudo systemctl start framelog.service
```

## Context sources (real hardware)

| Source | Signals |
|--------|---------|
| `power` | AC connected, input watts, adapter watts reported |
| `battery` | Status, percent, power |
| `display` | External monitor count |
| `profile` | Active power profile (D-Bus) |
| `sleep` | Sleep/wake markers |
| `pmf` | `spl_mw`, `sppt_mw`, `fppt_mw`, STT skin temps (debugfs) |
| `gpu_power` | dGPU runtime status, suspend, D3cold |

With `--collector fake`, synthetic sources exercise the UI without hardware.

## Framework issue #146 evidence

When filing or reviewing traces for [issue #146](https://github.com/FrameworkComputer/SoftwareFirmwareIssueTracker/issues/146), correlate:

- dGPU runtime (`gpu_power.dgpu_runtime_status`, `dgpu_runtime_suspended`)
- OS power profile (`profile.active`)
- Charger (`power.ac_connected`, `power.input_watts`, `power.adapter_watts_reported`)
- PMF limits (`pmf.*`) when debugfs is readable
- Throttle flags (`SPL`, `PROCHOT_CPU`) and APU power/temperature

For the reported `\_SB_.PCI0.GPP0.M241(1)` / `M241(0)` race trigger, see
[docs/framework-146-m241-trigger-capture.md](docs/framework-146-m241-trigger-capture.md).

For the **544/545 MHz (or ~1400 MHz) CPU frequency lock** after long idle, see
[docs/framework-146-long-idle-capture.md](docs/framework-146-long-idle-capture.md).
That workflow uses normal read-only collection; the explicit opt-in ACPI helper
is only for the M241 trigger path.

**PMF limits (live)** come from `/sys/kernel/debug/amd_pmf/current_power_limits`. **Full ACPI `STTC` tables** live in firmware AML; see [this discussion](https://github.com/FrameworkComputer/SoftwareFirmwareIssueTracker/issues/146#issuecomment-2908435584). `framelog` does not modify firmware or call `ryzenadj` from the hot path.

## Throttle flags

AMD SMU **independent throttle status** bits (same family as `amdgpu_top`):

- **Power:** `SPL`, `SPPT`, `FPPT`, `PPT0`–`PPT3`
- **Temperature:** `PROCHOT_CPU`, `PROCHOT_GPU`, `TEMP_HOTSPOT`, …
- **Current:** `TDC_*`, `EDC_*`

## System inventory

Harvested at service start and refreshed about daily. Shown in the UI and embedded in exports. Read-only from `/proc`, `/sys`, and `/etc/os-release`; storage serial numbers are omitted.

## API

REST API on the same port as the UI (`127.0.0.1:8787` by default).

**Health and bounds**

- `GET /api/health`
- `GET /api/capabilities` — detected hardware, signals, and Evidence Doctor rule availability
- `GET /api/time-bounds`, `GET /api/summary`

**Telemetry series**

- `GET /api/devices`, `GET /api/flags`
- `GET /api/series/{flag}`, `GET /api/metrics/{metric}`
- `GET /api/transitions`, `GET /api/transitions/journal?ids=`
- `GET /api/transitions/{id}/journal`

**Context**

- `GET /api/context/keys`, `GET /api/context/health`
- `GET /api/context/series/{key}`, `GET /api/context/transitions`
- `GET /api/context/transitions/journal?ids=`
- `GET /api/context/transitions/{id}/journal`

**Export and analysis**

- `GET /api/export` — JSON bundle for the requested window (**7 days** / **500k samples** max)
- `GET /api/findings` — Evidence Doctor report for a window (live DB)
- `POST /api/analyze/export` — analyze a client-loaded export JSON (no upload to a server)

**System inventory**

- `GET /api/system`, `POST /api/system/refresh`

Use `framelog dump` or `capture` for full-range export bundles without the web limits.

## License

MIT
