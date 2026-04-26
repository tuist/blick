#!/usr/bin/env bash
#MISE description="Build the workspace and fail on compilation warnings"
set -euo pipefail

RUSTFLAGS="-D warnings" cargo build --locked --all-targets
bazelisk build //...
