#!/usr/bin/env bash
set -eu
exec node "$(dirname "$0")/smoke.mjs"
