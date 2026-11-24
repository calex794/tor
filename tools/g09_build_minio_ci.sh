#!/usr/bin/env bash
# G-09 test fixture only: exact upstream fix for multiple HTTP responses.
set -euo pipefail
test "${GITHUB_ACTIONS:-}" = true
: "${RUNNER_TEMP:?requires disposable runner directory}"
test "$(go env GOVERSION)" = go1.24.8
export GOTOOLCHAIN=local
sha=52eee5a2f19ea8ef06bf2dd5ab64ab41424cb591
source_dir=$(mktemp -d "$RUNNER_TEMP/g09-minio-source.XXXXXX")
trap 'rm -rf -- "$source_dir"' EXIT
git init "$source_dir"
cd "$source_dir"
git remote add origin https://github.com/minio/minio.git
git fetch --depth=1 origin "$sha"
git checkout --detach "$sha"
test "$(git rev-parse HEAD)" = "$sha"
flags=$(go run buildscripts/gen-ldflags.go)
CGO_ENABLED=0 go build -mod=readonly -tags kqueue -trimpath -ldflags "$flags" -o "$RUNNER_TEMP/minio-g09-fixed" .
git diff --exit-code -- go.mod go.sum
"$RUNNER_TEMP/minio-g09-fixed" --version
"$RUNNER_TEMP/minio-g09-fixed" --version | grep -F "commit-id=$sha"
sha256sum "$RUNNER_TEMP/minio-g09-fixed"
