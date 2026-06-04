# Framework #146 M241 race trigger capture

This guide captures a framelog trace while deliberately triggering the reported
Framework AMD 544 / 545 MHz race:

```text
\_SB_.PCI0.GPP0.M241(1) -> short delay -> \_SB_.PCI0.GPP0.M241(0)
```

The trigger is based on the GitHub issue report that the bug is caused by a
race between consecutive calls to `M241(1)`, the EC PMF handler, and `M241(0)`.
A short delay such as `0.1s` reportedly reproduces the issue roughly 60% of the
time, while a longer delay such as `2s` usually does not.

## Safety

This is **not** part of normal framelog collection. The script writes directly
to `/proc/acpi/call`, which can invoke firmware methods. Use it only when:

- You are debugging Framework issue #146 or a closely related PMF power-limit
  bug.
- You understand that ACPI method calls are outside framelog's normal read-only
  design.
- You are prepared to reboot if the machine lands in a bad power-management
  state.

The collector remains read-only; only `scripts/trigger-m241-race.sh` performs
the ACPI writes.

## Prerequisites

Install and load `acpi_call`. On Arch Linux with the stock `linux` kernel:

```bash
sudo pacman -S acpi_call
sudo modprobe acpi_call
```

If `modprobe` says the module is not found for your running kernel, install the
DKMS package and matching kernel headers, then rebuild the module:

```bash
sudo pacman -S acpi_call-dkms linux-headers
sudo dkms autoinstall
sudo modprobe acpi_call
```

For other Arch kernels, use the matching headers package, for example
`linux-lts-headers`, `linux-zen-headers`, or the headers package for your custom
kernel.

Confirm `/proc/acpi/call` is available:

```bash
test -w /proc/acpi/call && echo "acpi_call ready"
```

Build and install the current framelog binary:

```bash
cargo build --release
sudo install -m755 target/release/framelog /usr/local/bin/framelog
```

Make the trigger script executable if your checkout did not preserve the mode:

```bash
chmod +x scripts/trigger-m241-race.sh
```

Dry-run the trigger first:

```bash
sudo scripts/trigger-m241-race.sh --dry-run --iterations 2 --delay 0.1
```

## If `M241` returns `AE_NOT_FOUND`

`AE_NOT_FOUND` means `/proc/acpi/call` is working, but the exact ACPI path does
not exist on your firmware as written. Stop triggering and discover the method
path first.

The Framework issue comment uses:

```text
\_SB_.PCI0.GPP0.M241
```

Some firmware revisions may place the same method under a different bridge or
scope. The GitHub reproducer path is from a specific reporter's ACPI namespace;
**your Insyde BIOS may not define `M241` at all**, which matches both `AE_NOT_FOUND`
from `acpi_call` and an empty search in decompiled tables.

Prepare tables once (needs root to read firmware blobs):

```bash
sudo pacman -S acpica
sudo mkdir -p /tmp/framelog-acpi
sudo cp /sys/firmware/acpi/tables/DSDT /tmp/framelog-acpi/
sudo cp /sys/firmware/acpi/tables/SSDT* /tmp/framelog-acpi/ 2>/dev/null || true
sudo chown -R "$USER:$USER" /tmp/framelog-acpi
```

Run the discovery helper (or search manually):

```bash
chmod +x scripts/discover-acpi-m241.sh
scripts/discover-acpi-m241.sh /tmp/framelog-acpi
```

Manual search in the **`.dsl` files only** (`iasl -d` writes `DSDT.dsl`, `SSDT1.dsl`, …):

```bash
cd /tmp/framelog-acpi
iasl -d -p dsl/ DSDT SSDT* 2>&1 | tail -3
grep -HnE 'Method \(M241|M241' dsl/*.dsl || echo "no M241 in ACPI"
grep -Hn 'GPP0' dsl/*.dsl | head -20
```

If `grep` prints **nothing** for `M241` (and `acpi_call` returns `AE_NOT_FOUND`),
stop using the trigger script on this machine. You cannot reproduce that exact
race without the method in your ACPI namespace.

**What to do instead for Framework #146 evidence:**

- Capture normal workload traces with `framelog capture` while the 544/545 MHz
  (or 1400 MHz) cap happens naturally.
- Note BIOS version, EC version if known, charger type, and power profile.
- Correlate SPL/SPPT, package power, PMF debugfs (`pmf.*`), and dGPU runtime in
  the export — that is still valid evidence even without `M241` calls.

If discovery **does** find `M241`, inspect the surrounding `Scope (...)` blocks
in that `.dsl` file to build the full path, then retry:

```bash
sudo scripts/trigger-m241-race.sh \
  --yes-i-understand \
  --method '\_SB_.PCI0.GPP0.M241' \
  --iterations 10 \
  --delay 0.1
```

Replace `--method` with the path you actually found.

## Capture workflow

Stop the normal service so the guided capture can own the database:

```bash
sudo systemctl stop framelog.service
```

Start a clean capture in one terminal. Use a duration long enough to include a
baseline, several trigger attempts, and post-trigger behavior:

```bash
framelog capture \
  --duration 5m \
  --issue framework-146-m241-race \
  --output-dir framelog-capture-m241-race
```

In a second terminal, wait 20 to 30 seconds so the capture has a baseline, then
run several short-delay trigger attempts:

```bash
sudo scripts/trigger-m241-race.sh \
  --yes-i-understand \
  --iterations 10 \
  --delay 0.1 \
  --cooldown 5
```

If you want a comparison run that should usually **not** trigger the bug, repeat
with a longer delay:

```bash
sudo scripts/trigger-m241-race.sh \
  --yes-i-understand \
  --iterations 5 \
  --delay 2 \
  --cooldown 5
```

Restart the normal service after capture completes:

```bash
sudo systemctl start framelog.service
```

## What the trigger logs

The script writes journal markers with tag `framelog-m241-trigger` before and
after each ACPI call. framelog already collects journal entries around throttle
and context transitions, so these markers help line up the trigger attempts with
changes in:

- `SPL`, `SPPT`, `FPPT`, `PROCHOT_CPU`, and `PROCHOT_GPU`
- APU package power and core temperature
- PMF limits (`pmf.spl_mw`, `pmf.sppt_mw`, `pmf.fppt_mw`) when debugfs is readable
- dGPU runtime state (`gpu_power.*`)
- OS power profile (`profile.active`)

You can also inspect only the trigger markers:

```bash
journalctl -t framelog-m241-trigger --since "10 minutes ago"
```

## Capture artifacts

The capture folder contains:

- `export.json` — load this in the dashboard with **Load export**
- `analysis.json` — deterministic Evidence Doctor output
- `report.md` — human-readable findings
- `summary.txt` — pasteable GitHub issue summary
- `system.json` — system inventory
- `pre_capture_backup.json` — previous database backup unless skipped

For issue comments, include `summary.txt` plus the exact trigger command you ran,
kernel version, BIOS version, charger type, and whether the system got stuck at
approximately `544/545 MHz` or `1400 MHz`.

## Interpreting the result

A useful trace has at least one of these:

- A clear power or clock cap appearing shortly after a trigger marker.
- `SPL` / `SPPT` / `FPPT` transitions around the trigger.
- PMF limits changing around the trigger.
- A mismatch between persistent throttle flags and actual temperature/power
  behavior.

If `PROCHOT_CPU` and `PROCHOT_GPU` are 100% active for the whole capture with no
transitions, read [PROCHOT_CPU / PROCHOT_GPU investigation](prochot-status-investigation.md).
On Radeon 780M this can be a sticky legacy `gpu_metrics` status pattern rather
than proof of constant emergency thermal throttling.

## When M241 is missing (`AE_NOT_FOUND`)

If `scripts/discover-acpi-m241.sh` finds no `M241` method, use a normal long-running capture
instead: [framework-146-long-idle-capture.md](framework-146-long-idle-capture.md). Evidence Doctor
will still flag sustained **545 MHz** or **1400 MHz** locks from live `cpu.*` context samples.
