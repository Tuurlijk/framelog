# Framework #146 long-idle capture

Use this workflow when you want evidence for the **CPU frequency lock** reported on Framework AMD
laptops after long idle (BIOS 4.04 community thread), separate from the **35W SPL cap** pattern
tracked in [Framework issue #146](https://github.com/FrameworkComputer/SoftwareFirmwareIssueTracker/issues/146).

## What framelog records

With a current build, the `cpu` context source samples sysfs cpufreq every tick:

- `cpu.cur_freq_min_mhz` — lowest `scaling_cur_freq` across online CPUs
- `cpu.cur_freq_avg_mhz` / `cpu.cur_freq_max_mhz`
- `cpu.governor`, `cpu.scaling_max_freq_mhz`

Evidence Doctor adds a **`cpu_frequency_lock`** finding when minimum frequency stays in:

- **544–545 MHz** band (≥50% of samples), or
- **~1400 MHz** band on some machines

That finding is **independent** of `framework_146_shape` (SPL + low package power + dGPU asleep).

## Recommended capture

1. Ensure the service is running with a build that includes the `cpu` context source:

   ```bash
   framelog capabilities   # should list context source "cpu"
   sudo systemctl restart framelog.service   # if using systemd
   ```

2. Reproduce the idle scenario (lid closed or open, on AC, dGPU asleep) for **at least 30–60 minutes**
   so cpufreq samples cover the stuck state.

3. Export or run guided capture:

   ```bash
   framelog capture --issue framework-146 --duration 90m
   # or export from the UI / API for the same window
   framelog analyze framelog-capture-framework-146-*/export.json
   ```

4. In the dashboard, chart **CPU min frequency (MHz)** (`cpu.cur_freq_min_mhz`) alongside
   package power and PMF SPL. A flat line near **545** or **1400** MHz with normal governor
   supports the frequency-lock hypothesis.

5. Attach `export.json` and `summary.txt` to the Framework thread or GitHub issue.

## M241 ACPI trigger (optional)

If your firmware exposes `\_SB_.PCI0.GPP0.M241`, you can try the controlled ACPI race script
documented in [framework-146-m241-trigger-capture.md](framework-146-m241-trigger-capture.md).

Many retail BIOS builds return **`AE_NOT_FOUND`** for M241; on those machines rely on this
long-idle capture instead of `scripts/trigger-m241-race.sh`.

## Related docs

- [PROCHOT sticky status](prochot-status-investigation.md) — do not confuse 100% PROCHOT with cpufreq lock
- [M241 trigger capture](framework-146-m241-trigger-capture.md) — optional firmware method probe
