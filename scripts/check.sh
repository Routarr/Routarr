#!/usr/bin/env bash
# What CI runs, locally, for the areas this branch changed: see scripts/check.py.
#
#   bash scripts/check.sh [--changed [BASE]] [--fail-fast] [--engines LIST] [--list] [AREA...]
set -euo pipefail
exec python3 "$(dirname "$0")/check.py" "$@"
