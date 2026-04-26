#!/usr/bin/env bash
#MISE description="Format the workspace"
set -euo pipefail

bazelisk run @rules_rust//:rustfmt
