#!/bin/bash
# Runs the app's unit tests on the first available iPad simulator.
set -euo pipefail
cd "$(dirname "$0")/.."
LIST=$(xcrun simctl list devices available)
SIM=$(awk '/^ +iPad/ {print; exit}' <<<"$LIST" | grep -oE '[0-9A-F]{8}(-[0-9A-F]{4}){3}-[0-9A-F]{12}' || true)
[[ -n "$SIM" ]] || { echo "no available iPad simulator: install an iOS Simulator runtime" >&2; exit 1; }
xcodebuild test -project Unveil.xcodeproj -scheme Unveil -destination "platform=iOS Simulator,id=$SIM" \
    -derivedDataPath "${DERIVED_DATA:-DerivedData}" -quiet
