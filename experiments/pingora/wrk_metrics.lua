-- No special headers or routes: this sends ordinary GET traffic.
function done(summary, latency, requests)
  io.write(string.format(
    "TOR_PINGORA_METRIC requests=%d p50_us=%.0f p95_us=%.0f p99_us=%.0f " ..
    "connect_errors=%d read_errors=%d write_errors=%d timeout_errors=%d status_errors=%d\n",
    summary.requests,
    latency:percentile(50),
    latency:percentile(95),
    latency:percentile(99),
    summary.errors.connect,
    summary.errors.read,
    summary.errors.write,
    summary.errors.timeout,
    summary.errors.status
  ))
end
