-- Requests are ordinary HTTP/1.1 GETs, with wrk's own keep-alive behavior.
function done(summary, latency, requests)
  local rps = 0
  if summary.duration > 0 then
    rps = summary.requests * 1000000 / summary.duration
  end
  io.write(string.format(
    "TOR_PINGORA_METRIC requests=%d duration_us=%d rps=%.2f " ..
    "p50_us=%.0f p95_us=%.0f p99_us=%.0f " ..
    "connect_errors=%d read_errors=%d write_errors=%d timeout_errors=%d status_errors=%d\n",
    summary.requests,
    summary.duration,
    rps,
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
