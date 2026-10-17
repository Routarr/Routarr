#!/usr/bin/env bash
# Says, at every start, which tool is missing or differs from the version its
# Dockerfile `ARG` pins. Never fails the start.
set -uo pipefail

DOCKERFILE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/Dockerfile"

pinned() { sed -n "s/^ARG $1_VERSION=\\(.*\\)$/\\1/p" "$DOCKERFILE"; }

stale=0
check() {
  local name="$1" arg="$2"
  shift 2
  local want got
  want="$(pinned "$arg")"
  [ -n "$want" ] || return 0
  if ! got="$("$@" 2>&1)"; then
    echo "tools: $name is missing (the Dockerfile pins $want): rebuild the container"
    stale=1
  elif ! grep -qF -- "$want" <<<"$got"; then
    echo "tools: $name answers \"$(head -1 <<<"$got")\", the Dockerfile pins $want: rebuild the container"
    stale=1
  fi
}

check node NODE node --version
check cargo-llvm-cov CARGO_LLVM_COV cargo llvm-cov --version
check cargo-about CARGO_ABOUT cargo about --version
check cargo-deny CARGO_DENY cargo-deny --version
check actionlint ACTIONLINT actionlint -version
check shellcheck SHELLCHECK shellcheck --version
check hadolint HADOLINT hadolint --version
check gitleaks GITLEAKS gitleaks version
check oasdiff OASDIFF oasdiff --version
check ruff RUFF ruff --version
check gh GH gh --version
# `cargo auditable --version` answers with cargo's own version, so only its
# presence can be read.
if [ -n "$(pinned CARGO_AUDITABLE)" ] && ! command -v cargo-auditable >/dev/null 2>&1; then
  echo "tools: cargo-auditable is missing (the Dockerfile pins $(pinned CARGO_AUDITABLE)): rebuild the container"
  stale=1
fi

if [ "$stale" = 1 ]; then
  echo "tools: VS Code, Dev Containers: Rebuild Container brings every tool to the version CI runs"
fi
exit 0
