#!/usr/bin/env bash
#MISE description="Build the workspace and fail on compilation warnings"
set -euo pipefail

bazelisk build //...
