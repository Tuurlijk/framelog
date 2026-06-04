/**
 * Live API and export-dump data sources for the framelog telemetry UI.
 */
(function (global) {
  const EXPORT_FORMAT = 'framelog_export';
  const METRIC_UNITS = {
    apu_power_mw: 'mW',
    stapm_limit_mw: 'mW',
    current_stapm_limit_mw: 'mW',
    temperature_core_max: '°C',
  };

  /** Evidence Doctor key_metrics: labels and value formatting (dashboard-aligned). */
  const KEY_METRIC_LABELS = {
    max_apu_power_mw: {
      label: 'Max package power',
      important: true,
      format(v) {
        const n = Number(v);
        return Number.isFinite(n) ? `${(n / 1000).toFixed(1)} W` : String(v);
      },
    },
    avg_apu_power_mw: {
      label: 'Avg package power',
      important: true,
      format(v) {
        const n = Number(v);
        return Number.isFinite(n) ? `${(n / 1000).toFixed(1)} W` : String(v);
      },
    },
    max_temperature_core_c: {
      label: 'Max core temp',
      important: true,
      format(v) {
        const n = Number(v);
        return Number.isFinite(n) ? `${n.toFixed(1)} °C` : String(v);
      },
    },
    spl_active_sample_pct: {
      label: 'SPL active (samples)',
      important: true,
      format(v) {
        const n = Number(v);
        return Number.isFinite(n) ? `${n.toFixed(1)}%` : String(v);
      },
    },
    pmf_spl_mw: {
      label: 'PMF SPL limit',
      important: true,
      format(v) {
        const n = Number(v);
        return Number.isFinite(n) ? `${(n / 1000).toFixed(1)} W` : String(v);
      },
    },
    dgpu_runtime_suspended_pct: {
      label: 'dGPU suspended',
      important: true,
      format(v) {
        const n = Number(v);
        return Number.isFinite(n) ? `${n.toFixed(1)}%` : String(v);
      },
    },
    ac_connected: {
      label: 'AC adapter',
      format(v) {
        if (v === true || v === 'true') return 'Connected';
        if (v === false || v === 'false') return 'On battery';
        return String(v);
      },
    },
    power_profile: {
      label: 'Power profile',
      format: v => String(v),
    },
    cpu_cur_freq_min_mhz: {
      label: 'CPU min freq (latest)',
      important: true,
      format(v) {
        const n = Number(v);
        return Number.isFinite(n) ? `${n.toFixed(0)} MHz` : String(v);
      },
    },
    cpu_freq_545_band_pct: {
      label: 'Time near 545 MHz lock',
      important: true,
      format(v) {
        const n = Number(v);
        return Number.isFinite(n) ? `${n.toFixed(1)}%` : String(v);
      },
    },
    cpu_freq_1400_band_pct: {
      label: 'Time near 1400 MHz lock',
      format(v) {
        const n = Number(v);
        return Number.isFinite(n) ? `${n.toFixed(1)}%` : String(v);
      },
    },
  };

  const CONTEXT_KEY_LABELS = {
    'power.ac_connected': 'AC adapter connected',
    'power.input_watts': 'Charger input (W)',
    'power.adapter_watts_reported': 'Adapter rating (W)',
    'pmf.spl_mw': 'PMF slow package limit (SPL)',
    'pmf.sppt_mw': 'PMF sustained package limit (SPPT)',
    'pmf.fppt_mw': 'PMF fast package limit (FPPT)',
    'pmf.stt_hs2_c': 'PMF skin temp limit (STT HS2)',
    'cpu.cur_freq_min_mhz': 'CPU min frequency (MHz)',
    'cpu.cur_freq_avg_mhz': 'CPU avg frequency (MHz)',
    'cpu.cur_freq_max_mhz': 'CPU max frequency (MHz)',
    'cpu.scaling_max_freq_mhz': 'CPU scaling cap (MHz)',
    'cpu.governor': 'CPU cpufreq governor',
    'gpu_power.dgpu_runtime_suspended': 'dGPU runtime suspended',
    'gpu_power.dgpu_runtime_status': 'dGPU runtime status',
    'battery.percent': 'Battery charge (%)',
    'display.external_count': 'External displays',
  };

  const KEY_METRIC_ORDER = [
    'max_apu_power_mw',
    'avg_apu_power_mw',
    'max_temperature_core_c',
    'spl_active_sample_pct',
    'pmf_spl_mw',
    'cpu_cur_freq_min_mhz',
    'cpu_freq_545_band_pct',
    'dgpu_runtime_suspended_pct',
    'ac_connected',
    'power_profile',
  ];

  function formatKeyMetric(key, rawValue) {
    const spec = KEY_METRIC_LABELS[key];
    const value = spec?.format ? spec.format(rawValue) : String(rawValue);
    const label = spec?.label ?? key.replace(/_/g, ' ');
    const important = spec?.important
      ?? (key.includes('spl') || key.includes('power') || key.includes('dgpu'));
    return { label, value, important };
  }

  function sortKeyMetricEntries(entries) {
    const rank = new Map(KEY_METRIC_ORDER.map((k, i) => [k, i]));
    return [...entries].sort(([a], [b]) => {
      const ra = rank.has(a) ? rank.get(a) : 999;
      const rb = rank.has(b) ? rank.get(b) : 999;
      if (ra !== rb) return ra - rb;
      return a.localeCompare(b);
    });
  }

  const FLAG_CATALOG = [
    ['SPL', 'power', 'Longer-term socket/package power limit is active'],
    ['SPPT', 'power', 'Sustained package power limit is active'],
    ['FPPT', 'power', 'Short burst package power limit is active'],
    ['SPPT_APU', 'power', 'Sustained APU package power limit is active'],
    ['PPT0', 'power', 'Firmware power budget limit 0 is active'],
    ['PPT1', 'power', 'Firmware power budget limit 1 is active'],
    ['PPT2', 'power', 'Firmware power budget limit 2 is active'],
    ['PPT3', 'power', 'Firmware power budget limit 3 is active'],
    ['PROCHOT_CPU', 'thermal', 'CPU hot/throttle signal is active'],
    ['PROCHOT_GPU', 'thermal', 'GPU hot/throttle signal is active'],
    ['TEMP_HOTSPOT', 'thermal', 'Hottest sensor has reached its limit'],
    ['TEMP_CORE', 'thermal', 'CPU/APU core temperature limit is active'],
    ['TEMP_GPU', 'thermal', 'GPU temperature limit is active'],
    ['TEMP_EDGE', 'thermal', 'GPU edge temperature limit is active'],
    ['TEMP_MEM', 'thermal', 'Graphics memory temperature limit is active'],
    ['TEMP_SOC', 'thermal', 'System-on-chip temperature limit is active'],
    ['TEMP_VR_GFX', 'thermal', 'Graphics voltage regulator temperature limit is active'],
    ['TEMP_VR_SOC', 'thermal', 'SoC voltage regulator temperature limit is active'],
    ['TEMP_VR_MEM0', 'thermal', 'Memory voltage regulator 0 temperature limit is active'],
    ['TEMP_VR_MEM1', 'thermal', 'Memory voltage regulator 1 temperature limit is active'],
    ['TEMP_LIQUID0', 'thermal', 'Liquid-cooling sensor 0 temperature limit is active'],
    ['TEMP_LIQUID1', 'thermal', 'Liquid-cooling sensor 1 temperature limit is active'],
    ['VRHOT0', 'thermal', 'Voltage regulator hot signal 0 is active'],
    ['VRHOT1', 'thermal', 'Voltage regulator hot signal 1 is active'],
    ['TDC_GFX', 'current', 'Sustained graphics current limit is active'],
    ['TDC_SOC', 'current', 'Sustained SoC current limit is active'],
    ['TDC_MEM', 'current', 'Sustained memory current limit is active'],
    ['TDC_VDD', 'current', 'Sustained core-voltage current limit is active'],
    ['TDC_CVIP', 'current', 'Sustained CVIP current limit is active'],
    ['EDC_CPU', 'current', 'Short spike CPU current limit is active'],
    ['EDC_GFX', 'current', 'Short spike graphics current limit is active'],
    ['APCC', 'power', 'Firmware platform-control power limit is active'],
    ['PPM', 'power', 'Platform power management limit is active'],
    ['FIT', 'power', 'Reliability/aging protection limit is active'],
  ].map(([name, category, description]) => ({ name, category, description }));

  function downsampleByStride(points, maxPoints) {
    if (!maxPoints || points.length <= maxPoints) return points;
    const stride = Math.ceil(points.length / maxPoints);
    const out = [];
    for (let i = 0; i < points.length; i += stride) out.push(points[i]);
    const last = points[points.length - 1];
    if (out[out.length - 1] !== last) out.push(last);
    return out;
  }

  function normalizeTemperature(metric, value) {
    if (metric === 'temperature_core_max' && value > 1000) return value / 100;
    return value;
  }

  function inRange(ts, from, to) {
    return ts >= from && ts <= to;
  }

  function contextKeyLabel(key) {
    return CONTEXT_KEY_LABELS[key] ?? key;
  }

  function contextKeyKind(key) {
    const numericSuffixes = [
      '.percent', '.watts', '_watts', '_mw', '_ms', '_c', '_mhz',
      '.ac_connected', '.external_connected', '.external_count', '.input_watts',
      '.adapter_watts_reported', '.dgpu_runtime_suspended', '.dgpu_d3cold_allowed',
      '.online_cpus',
    ];
    return numericSuffixes.some(s => key.endsWith(s)) ? 'number' : 'enum';
  }

  function contextStrToNum(s) {
    const lower = String(s).toLowerCase();
    if (lower === 'true' || lower === 'on' || lower === 'yes' || lower === 'connected') return 1;
    if (lower === 'false' || lower === 'off' || lower === 'no' || lower === 'disconnected') return 0;
    const n = Number(s);
    return Number.isFinite(n) ? n : 0;
  }

  function parseExportBundle(raw) {
    if (!raw || typeof raw !== 'object') {
      throw new Error('Invalid export: expected JSON object');
    }
    if (raw.format && raw.format !== EXPORT_FORMAT) {
      throw new Error(`Unsupported export format: ${raw.format}`);
    }
    const version = raw.schema_version ?? 1;
    if (version !== 1) {
      throw new Error(`Unsupported export schema_version: ${version}`);
    }
    if (!Array.isArray(raw.samples)) {
      throw new Error('Invalid export: missing samples array');
    }
    return raw;
  }

  class ApiDataSource {
    constructor(fetchJson) {
      this._fetch = fetchJson;
      this.mode = 'live';
    }

    label() {
      return 'Live database';
    }

    async timeBounds() {
      return this._fetch('/api/time-bounds');
    }

    async devices() {
      return this._fetch('/api/devices');
    }

    async flags() {
      return this._fetch('/api/flags');
    }

    async summary(opts) {
      const params = buildParams(opts);
      return this._fetch(`/api/summary?${params}`);
    }

    async series(flag, opts) {
      const params = buildParams(opts);
      return this._fetch(`/api/series/${encodeURIComponent(flag)}?${params}`);
    }

    async metrics(metric, opts) {
      const params = buildParams(opts);
      return this._fetch(`/api/metrics/${encodeURIComponent(metric)}?${params}`);
    }

    async transitions(opts) {
      const params = buildParams(opts);
      return this._fetch(`/api/transitions?${params}`);
    }

    async transitionJournal(ids) {
      if (!ids.length) return [];
      return this._fetch(`/api/transitions/journal?ids=${ids.join(',')}`);
    }

    async contextKeys() {
      return this._fetch('/api/context/keys');
    }

    async contextSeries(key, opts) {
      const params = buildParams(opts);
      return this._fetch(`/api/context/series/${encodeURIComponent(key)}?${params}`);
    }

    async contextTransitions(opts) {
      const params = buildParams(opts);
      return this._fetch(`/api/context/transitions?${params}`);
    }

    async contextJournal(ids) {
      if (!ids.length) return [];
      return this._fetch(`/api/context/transitions/journal?ids=${ids.join(',')}`);
    }

    async contextHealth() {
      return this._fetch('/api/context/health');
    }

    async system() {
      return this._fetch('/api/system');
    }

    async refreshSystem() {
      return this._fetch('/api/system/refresh', { method: 'POST' });
    }

    async findings(opts) {
      const params = buildParams(opts);
      return this._fetch(`/api/findings?${params}`);
    }
  }

  class DumpDataSource {
    constructor(bundle) {
      this.mode = 'dump';
      this.bundle = parseExportBundle(bundle);
      this._throttleJournal = groupJournal(bundle.throttle_journal_events || []);
      this._contextJournal = groupJournal(bundle.context_journal_events || []);
    }

    label() {
      const b = this.bundle;
      return `Export ${fmtExportRange(b.from_ms, b.to_ms)}`;
    }

    exportMeta() {
      const b = this.bundle;
      return {
        from_ms: b.from_ms,
        to_ms: b.to_ms,
        exported_at_ms: b.exported_at_ms,
        device_pci_filter: b.device_pci_filter,
      };
    }

    timeBounds() {
      const samples = this.bundle.samples || [];
      const ctx = this.bundle.context_snapshots || [];
      let min = null;
      let max = null;
      for (const s of samples) {
        if (min == null || s.ts_unix_ms < min) min = s.ts_unix_ms;
        if (max == null || s.ts_unix_ms > max) max = s.ts_unix_ms;
      }
      for (const c of ctx) {
        if (min == null || c.ts_unix_ms < min) min = c.ts_unix_ms;
        if (max == null || c.ts_unix_ms > max) max = c.ts_unix_ms;
      }
      return {
        min_ts_ms: min,
        max_ts_ms: max,
        sample_count: samples.length,
        context_snapshot_count: ctx.length,
      };
    }

    devices() {
      const devices = this.bundle.devices || [];
      return { devices };
    }

    flags() {
      const observed = new Set();
      for (const s of this.bundle.samples || []) {
        for (const f of s.active_flags || []) observed.add(f);
      }
      for (const t of this.bundle.throttle_transitions || []) {
        observed.add(t.flag_name);
      }
      const flags = [...observed].sort();
      return { flags, catalog: FLAG_CATALOG };
    }

    summary(opts) {
      const { from, to, device_pci } = opts;
      const samples = this._samplesInRange(from, to, device_pci);
      const transitions = this._throttleTransitionsInRange(from, to, device_pci, null, null);
      const ctxTransitions = this._contextTransitionsInRange(from, to, null, null);

      const flagStats = new Map();
      for (const t of transitions) {
        const entry = flagStats.get(t.flag_name) || { assert: 0, clear: 0 };
        if (t.new_value) entry.assert += 1;
        else entry.clear += 1;
        flagStats.set(t.flag_name, entry);
      }

      const flagActiveSamples = new Map();
      for (const s of samples) {
        for (const flag of s.active_flags || []) {
          flagActiveSamples.set(flag, (flagActiveSamples.get(flag) || 0) + 1);
        }
      }
      for (const flag of flagActiveSamples.keys()) {
        if (!flagStats.has(flag)) flagStats.set(flag, { assert: 0, clear: 0 });
      }

      const sampleCount = samples.length;
      const flag_activity = [...flagStats.entries()]
        .map(([flag_name, counts]) => {
          const active = flagActiveSamples.get(flag_name) || 0;
          const pct = sampleCount > 0 ? (active / sampleCount) * 100 : 0;
          return {
            flag_name,
            active_sample_pct: pct,
            assert_count: counts.assert,
            clear_count: counts.clear,
          };
        })
        .sort((a, b) => a.flag_name.localeCompare(b.flag_name));

      const apuPowers = samples.map(s => s.apu_power_mw).filter(v => v != null);
      const max_apu_power_mw = apuPowers.length ? Math.max(...apuPowers) : null;
      const avg_apu_power_mw = apuPowers.length
        ? Math.floor(apuPowers.reduce((a, b) => a + b, 0) / apuPowers.length)
        : null;

      let max_temperature_core = null;
      for (const s of samples) {
        const t = s.temperature_core_max;
        if (t == null) continue;
        const n = normalizeTemperature('temperature_core_max', t);
        if (max_temperature_core == null || n > max_temperature_core) max_temperature_core = n;
      }

      return {
        from_ms: from,
        to_ms: to,
        sample_count: sampleCount,
        throttle_transition_count: transitions.length,
        context_transition_count: ctxTransitions.length,
        flag_activity,
        max_apu_power_mw,
        avg_apu_power_mw,
        max_temperature_core,
      };
    }

    series(flag, opts) {
      const { from, to, device_pci, max_points } = opts;
      let points = this._samplesInRange(from, to, device_pci)
        .map(s => ({
          ts_unix_ms: s.ts_unix_ms,
          value: (s.active_flags || []).includes(flag) ? 1 : 0,
        }));
      if (max_points) points = downsampleByStride(points, max_points);
      return { flag, points };
    }

    metrics(metric, opts) {
      const { from, to, device_pci, max_points } = opts;
      const field = metricField(metric);
      if (!field) throw new Error(`unknown metric: ${metric}`);
      let points = this._samplesInRange(from, to, device_pci)
        .map(s => {
          const raw = s[field];
          if (raw == null) return null;
          const value = normalizeTemperature(metric, Number(raw));
          return { ts_unix_ms: s.ts_unix_ms, value };
        })
        .filter(Boolean);
      if (max_points) points = downsampleByStride(points, max_points);
      return {
        metric,
        unit: METRIC_UNITS[metric] || '',
        points,
      };
    }

    transitions(opts) {
      const {
        from, to, device_pci, flag_name, direction, limit = 100,
      } = opts;
      let items = this._throttleTransitionsInRange(from, to, device_pci, flag_name, direction);
      items.sort((a, b) => b.ts_unix_ms - a.ts_unix_ms);
      return items.slice(0, limit);
    }

    transitionJournal(ids) {
      const out = [];
      for (const id of ids) {
        const list = this._throttleJournal.get(id) || [];
        out.push(...list);
      }
      return out;
    }

    contextKeys() {
      const keys = new Set();
      for (const row of this.bundle.context_values || []) keys.add(row.key);
      return { keys: [...keys].sort() };
    }

    contextSeries(key, opts) {
      const { from, to, max_points } = opts;
      const value_kind = contextKeyKind(key);
      const rows = (this.bundle.context_values || [])
        .filter(r => r.key === key && inRange(r.ts_unix_ms, from, to))
        .sort((a, b) => a.ts_unix_ms - b.ts_unix_ms);

      const enum_labels = {};
      const points = [];
      for (const r of rows) {
        let value;
        if (value_kind === 'number') {
          value = r.value_num ?? (r.value_str != null ? contextStrToNum(r.value_str) : null);
          if (value == null) continue;
        } else {
          const label = r.value_str ?? String(r.value_num ?? '');
          value = contextStrToNum(label);
          enum_labels[label] = value;
        }
        points.push({ ts_unix_ms: r.ts_unix_ms, value });
      }

      const down = max_points ? downsampleByStride(points, max_points) : points;
      return {
        key,
        value_kind,
        enum_labels: Object.keys(enum_labels).length ? enum_labels : null,
        points: down,
      };
    }

    contextTransitions(opts) {
      const { from, to, limit = 100 } = opts;
      let items = this._contextTransitionsInRange(from, to, null, null);
      items.sort((a, b) => b.ts_unix_ms - a.ts_unix_ms);
      return items.slice(0, limit);
    }

    contextJournal(ids) {
      const out = [];
      for (const id of ids) {
        const list = this._contextJournal.get(id) || [];
        out.push(...list);
      }
      return out;
    }

    contextHealth() {
      const latest = new Map();
      for (const s of this.bundle.context_snapshots || []) {
        const prev = latest.get(s.source_id);
        if (!prev || s.ts_unix_ms >= prev.ts_unix_ms) {
          latest.set(s.source_id, {
            source_id: s.source_id,
            ts_unix_ms: s.ts_unix_ms,
            health: s.health,
            error_message: s.error_message,
          });
        }
      }
      return [...latest.values()].sort((a, b) => a.source_id.localeCompare(b.source_id));
    }

    system() {
      return this.bundle.system ?? null;
    }

    refreshSystem() {
      return this.system();
    }

    async findings(opts) {
      const { from, to, device_pci } = opts;
      const res = await fetch('/api/analyze/export', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          bundle: this.bundle,
          from_ms: from,
          to_ms: to,
          device_pci,
        }),
      });
      if (!res.ok) {
        const text = await res.text();
        throw new Error(text || res.statusText);
      }
      return res.json();
    }

    mergedEvents(opts) {
      const { from, to, device_pci, flagNames } = opts;
      const events = [];
      const flagSet = flagNames?.length ? new Set(flagNames) : null;

      for (const t of this._throttleTransitionsInRange(from, to, device_pci, null, null)) {
        if (flagSet && !flagSet.has(t.flag_name)) continue;
        events.push({
          ts: t.ts_unix_ms,
          type: 'throttle',
          id: t.id,
          summary: `${t.flag_name} ${t.new_value ? 'asserted' : 'cleared'}`,
          source: t.device_pci,
          journalCount: (this._throttleJournal.get(t.id) || []).length,
        });
      }

      for (const t of this._contextTransitionsInRange(from, to, null, null)) {
        events.push({
          ts: t.ts_unix_ms,
          type: 'context',
          id: t.id,
          summary: `${t.key}: ${jsonBrief(t.old_value)} → ${jsonBrief(t.new_value)}`,
          source: t.source_id,
          journalCount: (this._contextJournal.get(t.id) || []).length,
        });
      }

      events.sort((a, b) => a.ts - b.ts);
      return events;
    }

    _samplesInRange(from, to, device_pci) {
      return (this.bundle.samples || []).filter(s => {
        if (!inRange(s.ts_unix_ms, from, to)) return false;
        if (device_pci && s.device_pci !== device_pci) return false;
        return true;
      });
    }

    _throttleTransitionsInRange(from, to, device_pci, flag_name, direction) {
      return (this.bundle.throttle_transitions || []).filter(t => {
        if (!inRange(t.ts_unix_ms, from, to)) return false;
        if (device_pci && t.device_pci !== device_pci) return false;
        if (flag_name && t.flag_name !== flag_name) return false;
        if (direction === 'asserted' && !t.new_value) return false;
        if (direction === 'cleared' && t.new_value) return false;
        return true;
      });
    }

    _contextTransitionsInRange(from, to, source_id, key) {
      return (this.bundle.context_transitions || []).filter(t => {
        if (!inRange(t.ts_unix_ms, from, to)) return false;
        if (source_id && t.source_id !== source_id) return false;
        if (key && t.key !== key) return false;
        return true;
      });
    }
  }

  function buildParams(opts) {
    const params = new URLSearchParams();
    if (opts.from != null) params.set('from', String(opts.from));
    if (opts.to != null) params.set('to', String(opts.to));
    if (opts.device_pci) params.set('device_pci', opts.device_pci);
    if (opts.max_points != null) params.set('max_points', String(opts.max_points));
    if (opts.limit != null) params.set('limit', String(opts.limit));
    if (opts.flag_name) params.set('flag_name', opts.flag_name);
    if (opts.direction) params.set('direction', opts.direction);
    return params;
  }

  function groupJournal(events) {
    const map = new Map();
    for (const e of events) {
      const list = map.get(e.transition_id) || [];
      list.push(e);
      map.set(e.transition_id, list);
    }
    return map;
  }

  function metricField(metric) {
    const allowed = new Set([
      'apu_power_mw', 'stapm_limit_mw', 'current_stapm_limit_mw', 'temperature_core_max',
    ]);
    return allowed.has(metric) ? metric : null;
  }

  function jsonBrief(v) {
    if (v == null) return '—';
    if (typeof v === 'string') return v;
    return JSON.stringify(v);
  }

  function fmtExportRange(from, to) {
    const f = new Date(from).toLocaleString();
    const t = new Date(to).toLocaleString();
    return `${f} – ${t}`;
  }

  global.FramelogDataSources = {
    ApiDataSource,
    DumpDataSource,
    parseExportBundle,
    EXPORT_FORMAT,
    FLAG_CATALOG,
    KEY_METRIC_LABELS,
    CONTEXT_KEY_LABELS,
    contextKeyLabel,
    formatKeyMetric,
    sortKeyMetricEntries,
  };
})(typeof window !== 'undefined' ? window : globalThis);
