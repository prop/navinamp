#!/usr/bin/env bash
# Register this Mac as a self-hosted GitHub Actions runner for NaVinaMP.
#
# Prereqs:
#   1. Repo → Settings → Actions → Runners → New self-hosted runner → macOS → ARM64
#   2. Copy the RUNNER_TOKEN from that page and the actions-runner tarball URL.
#
# Usage:
#   ./scripts/bootstrap-github-runner.sh <RUNNER_TOKEN> [TARBALL_URL]
#
# Then start the workflow once (push to master or Actions → Run workflow) and
# watch the "Release build (Mac mini)" job pick up this machine.
#
# SECURITY: the repo is public — the macOS job is gated to push/dispatch events
# ONLY (see .github/workflows/ci.yml). Fork pull requests never run here.
set -euo pipefail

TOKEN="${1:?usage: $0 <RUNNER_TOKEN> [TARBALL_URL]}"
URL="${2:-}"
DIR="$HOME/actions-runner"
WORK="$HOME/actions-runner-work"
REPO_URL="https://github.com/prop/NaVinaMP"

if [ ! -x "$DIR/config.sh" ]; then
  if [ -z "$URL" ]; then
    echo "No runner installed and no TARBALL_URL given."
    echo "Copy the download URL from Settings → Actions → Runners → New self-hosted runner."
    exit 1
  fi
  mkdir -p "$DIR"
  cd "$DIR"
  curl -L -o runner.tar.gz "$URL"
  tar xzf runner.tar.gz
else
  echo "Using existing runner install at $DIR"
fi

cd "$DIR"
./config.sh \
  --url "$REPO_URL" \
  --token "$TOKEN" \
  --name mac-mini \
  --labels macOS \
  --work "$WORK" \
  --unattended \
  --replace

# Run as a launchd service so it survives reboots (run these two with sudo
# if the service needs to run as root instead of this user):
./svc.sh install
./svc.sh start

echo "Runner registered and started. Remove with: cd $DIR && ./config.sh remove --token <TOKEN>"
