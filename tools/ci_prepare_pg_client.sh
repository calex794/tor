#!/usr/bin/env bash
# CI runner only: use the existing PostgreSQL client or install the missing tools.
set -e

if command -v psql >/dev/null 2>&1 && command -v pg_isready >/dev/null 2>&1; then
  psql --version
  echo "Using preinstalled PostgreSQL client"
else
  sudo apt-get update -qq
  sudo apt-get install -y postgresql-client
fi
