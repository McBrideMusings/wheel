#!/bin/bash
# Runs the wheelhouse plugin's pane-script tests (wheel-pmf2.8): plugins/wheelhouse/panes/beads.luau,
# diff.luau and lib/*.luau loaded into WheelLuau's PaneRuntime against fixture feeds, with the messages
# a client sends, asserting on the flattened tree. Uses no daemon, GUI, focus or network.
#
#   tools/wheelhouse-panes.sh          # the whole suite
#   tools/wheelhouse-panes.sh <filter> # tests whose name contains <filter>
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
FILTER=${1:-WheelhousePaneTests}
exec swift test --package-path "$ROOT/apple/WheelLuau" --filter "$FILTER"
