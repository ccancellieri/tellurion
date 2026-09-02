// Minimal Prometheus text-format parser — deliberately not a library:
// `/metrics` (see the server's `metrics_handler`) only ever needs 2-3
// counters read out of it here, and the exposition format
// (https://github.com/prometheus/docs/blob/main/content/docs/instrumenting/exposition_formats.md)
// is simple enough that a full client library would be overkill for a demo
// status widget.

export interface ParsedMetrics {
  totalRequests: number;
  avgLatencyMs: number | null;
  residentMemoryBytes: number | null;
}

// Matches one exposition line: `metric_name{label="value",...} 1.23`, or
// `metric_name 1.23` when there are no labels. Comment lines (`# HELP`,
// `# TYPE`) never match and are skipped by the caller before this runs.
const METRIC_LINE = /^([a-zA-Z_:][a-zA-Z0-9_:]*)(\{[^}]*\})?\s+([0-9eE+\-.]+)$/;

export function parsePrometheusText(text: string): ParsedMetrics {
  let requestCountSum = 0;
  let requestDurationSum = 0;
  let residentMemoryBytes: number | null = null;

  for (const rawLine of text.split('\n')) {
    const line = rawLine.trim();
    if (line === '' || line.startsWith('#')) continue;

    const match = METRIC_LINE.exec(line);
    if (!match) continue;
    const name = match[1];
    const value = Number(match[3]);
    if (!Number.isFinite(value)) continue;

    switch (name) {
      case 'http_request_duration_seconds_count':
        requestCountSum += value;
        break;
      case 'http_request_duration_seconds_sum':
        requestDurationSum += value;
        break;
      case 'process_resident_memory_bytes':
        // A single gauge, no labels — only ever one line, but assigning
        // rather than summing is correct either way.
        residentMemoryBytes = value;
        break;
      default:
        break;
    }
  }

  return {
    totalRequests: requestCountSum,
    avgLatencyMs: requestCountSum > 0 ? (requestDurationSum / requestCountSum) * 1000 : null,
    residentMemoryBytes,
  };
}
