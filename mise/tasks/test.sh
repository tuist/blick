#!/usr/bin/env bash
#MISE description="Run the project test suites"
set -euo pipefail

cargo test
bazelisk test //...
shellspec
