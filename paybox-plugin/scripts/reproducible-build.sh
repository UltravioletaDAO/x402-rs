#!/usr/bin/env bash
# Builds dist/plugin.mjs from a clean export of a commit, inside node:24.21.0 (the starter's
# reference toolchain, linux), and prints the starter's report (artifact.bytes, artifact.sha256), the
# unit tests and the bundle check. The network is used by the install and by ONE read: the
# conformance's sample call is x402_supported against the facilitator (paybox-plugin.json), a public
# read that signs and moves nothing. The unit tests replace fetch with one that throws, or point it
# at a fake MCP on 127.0.0.1.
# The bytes must equal the ones `pnpm test` reports anywhere: the build is deterministic (the
# starter's own test builds twice), and the export is LF whatever core.autocrlf says.
#
#   bash paybox-plugin/scripts/reproducible-build.sh [commit] [out-dir]   # default HEAD, no copy
set -euo pipefail
commit="${1:-HEAD}"
copy_to="${2:-}"
repo="$(git rev-parse --show-toplevel)"
out="$(mktemp -d "${TMPDIR:-/tmp}/uvd-facilitator-paybox-plugin.XXXXXX")"
trap 'rm -rf "$out"' EXIT
git -C "$repo" -c core.autocrlf=false archive "$commit" paybox-plugin | tar -x -C "$out"
# node:24.21.0 por su digest (el indice multiplataforma, el mismo que fija el plugin de Emporium):
# el mismo en amd64 y arm64. PLATFORM=linux/amd64 lo corre en esa arquitectura (los bytes no cambian).
image="node:24.21.0@sha256:3d27e5c11e5786e309ec3e03f93ae536eb36e6e5eb3714d5eb3300a36157add0"
docker run --rm ${PLATFORM:+--platform "$PLATFORM"} -v "$out/paybox-plugin:/w" -w /w "$image" bash -c '
  set -eo pipefail
  npm install --global pnpm@11.11.0 >/dev/null 2>&1
  pnpm install --frozen-lockfile >/dev/null 2>&1
  pnpm --silent test 2>/dev/null
  pnpm --silent run test:unit 2>&1 | tail -9
  pnpm --silent run check:bundle'
if [ -n "$copy_to" ]; then
  mkdir -p "$copy_to"
  cp "$out/paybox-plugin/dist/plugin.mjs" "$copy_to/plugin.mjs"
fi
