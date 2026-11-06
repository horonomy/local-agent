#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -eq 0 ]]; then
  echo "usage: $0 <package> [package ...]" >&2
  exit 2
fi

apt_attempt_timeout_seconds=120
apt_install_timeout_seconds="${ZEROCLAW_CI_APT_INSTALL_TIMEOUT_SECONDS:-120}"
case "$apt_install_timeout_seconds" in
    ''|*[!0-9]*|????*)
        echo "install timeout must be an integer from 1 through 240 seconds" >&2
        exit 2
        ;;
esac
if ((10#$apt_install_timeout_seconds < 1 || 10#$apt_install_timeout_seconds > 240)); then
    echo "install timeout must be an integer from 1 through 240 seconds" >&2
    exit 2
fi
apt_max_attempts=2
apt_retry_delay_seconds=5

run_apt_phase() {
  local phase="$1"
  local attempt
  local outcome
  local status=1
  local phase_timeout="$apt_attempt_timeout_seconds"
  if [[ "$phase" == install ]]; then
    phase_timeout="$apt_install_timeout_seconds"
  fi
  shift

  for ((attempt = 1; attempt <= apt_max_attempts; attempt++)); do
    echo "apt ${phase}: attempt ${attempt}/${apt_max_attempts} started (deadline: ${phase_timeout}s)"

    set +e
    sudo timeout --signal=TERM --kill-after=10s \
      "${phase_timeout}s" apt-get "$@"
    status=$?
    set -e

    if [[ "$status" -eq 0 ]]; then
      echo "apt ${phase}: attempt ${attempt}/${apt_max_attempts} succeeded"
      return 0
    fi

    case "$status" in
      124|137) outcome="timed out" ;;
      *) outcome="failed" ;;
    esac
    echo "apt ${phase}: attempt ${attempt}/${apt_max_attempts} ${outcome} (status: ${status})" >&2

    if [[ "$attempt" -lt "$apt_max_attempts" ]]; then
      echo "apt ${phase}: retrying after ${apt_retry_delay_seconds}s" >&2
      sleep "$apt_retry_delay_seconds"
    fi
  done

  echo "apt ${phase}: failed after ${apt_max_attempts} attempts (last status: ${status})" >&2
  return "$status"
}

# GitHub-hosted Ubuntu images sometimes ship transient Microsoft apt sources.
# They are unrelated to this workflow and can break apt-get update before CI
# installs its own packages.
# Contract tests point the directory at a temporary tree; CI uses the default.
apt_sources_dir="${ZEROCLAW_CI_APT_SOURCES_DIR:-/etc/apt/sources.list.d}"
apt_sources=(
  "${apt_sources_dir}"/azure-cli.*
  "${apt_sources_dir}"/microsoft-prod.*
)

for apt_source in "${apt_sources[@]}"; do
  if [[ -e "$apt_source" || -L "$apt_source" ]]; then
    echo "Removing runner-provided apt source: $apt_source"
    sudo rm -f "$apt_source"
  fi
done

run_apt_phase refresh update -qq
run_apt_phase install install -y "$@"
