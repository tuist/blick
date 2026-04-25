#!/usr/bin/env bash
#MISE description="Format the workspace"
set -euo pipefail

cargo fmt --all
