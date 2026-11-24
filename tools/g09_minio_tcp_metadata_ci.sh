#!/usr/bin/env bash
# Test-only transport diagnostic for the disposable MinIO provider suite.
set -euo pipefail

trace_pid=
trace="$RUNNER_TEMP/tor-g09-tcp-metadata.log"
capture_status="$RUNNER_TEMP/tor-g09-tcp-capture-status.log"
if command -v tcpdump >/dev/null && sudo -n true; then
  # Text summaries only: never persist packets, HTTP headers or payloads.
  # Use the same Unix wall clock as G09_FIRST_GET_WINDOW in the test.
  sudo -n timeout --signal=TERM --kill-after=2s 1700s \
    tcpdump -i lo -nn -tt -l -s 96 -c 8192 \
      'ip and tcp and src host 127.0.0.1 and dst host 127.0.0.1 and port 9000 and (tcp[tcpflags] & (tcp-syn|tcp-fin|tcp-rst) != 0)' \
      > "$trace" 2> "$capture_status" &
  trace_pid=$!
  stop_trace() {
    sudo -n kill -TERM "$trace_pid" 2>/dev/null || true
    wait "$trace_pid" 2>/dev/null || true
  }
  trap stop_trace EXIT
  capture_state=not-ready
  for _ in $(seq 1 20); do
    if grep -Eq '^(tcpdump: )?listening on lo,' "$capture_status"; then
      capture_state=ready
      break
    fi
    if ! kill -0 "$trace_pid" 2>/dev/null; then break; fi
    sleep 0.1
  done
  echo "G09_TCP_METADATA=$capture_state"
else
  echo "G09_TCP_METADATA=unavailable"
fi
test_log="$RUNNER_TEMP/tor-g09-private-provider.log"
status=0
cargo test -p tor-storage --features s3 --test s3_private_provider -- --ignored --test-threads=1 \
  2>&1 | tee "$test_log" || status=$?
if [ -n "$trace_pid" ]; then stop_trace; trap - EXIT; fi
packet_count=0
if [ -s "$trace" ]; then
  packet_count=$(awk '/^[0-9]+\.[0-9]+ IP 127\.0\.0\.1\./ {count++} END {print count+0}' "$trace")
fi
echo "G09_TCP_METADATA control_packets=$packet_count"
if [ "$status" -ne 0 ]; then
  if [ -s "$trace" ]; then
    # Test harness may print a failed test after other tests have run.
    # Filter by the timestamp recorded inside the strict GET itself.
    awk '
      FILENAME == ARGV[1] {
        if (match($0, /G09_FIRST_GET_WINDOW unix_ms_start=[0-9]+ unix_ms_end=[0-9]+/)) {
          split(substr($0, RSTART, RLENGTH), mark, " ")
          split(mark[2], start, "=")
          split(mark[3], end, "=")
          lower[++n] = start[2] / 1000 - 1
          upper[n] = end[2] / 1000 + 1
        }
        next
      }
      /^[0-9]+\.[0-9]+ IP 127\.0\.0\.1\./ {
        for (i = 1; i <= n; i++) {
          if ($1 >= lower[i] && $1 <= upper[i]) {
            print
            break
          }
        }
      }
    ' "$test_log" "$trace" | head -n 240 || true
  fi
  if [ -f "$capture_status" ]; then tail -n 8 "$capture_status"; fi
  exit "$status"
fi
