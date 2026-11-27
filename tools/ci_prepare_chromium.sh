#!/usr/bin/env bash
# Shared CI setup; callers retain their own optional OS dependency fallback.
set -e

if [[ $# != 1 || ( "$1" != install && "$1" != install-and-probe ) ]]; then
  echo "Usage: $0 install|install-and-probe" >&2
  exit 2
fi

npm install --no-save playwright@1.56.1
npx playwright install --only-shell chromium

if [[ "$1" == install-and-probe ]]; then
  node -e "require('playwright').chromium.launch({headless:true}).then(b=>b.close()).catch(e=>{console.error(e);process.exit(1)})"
fi
