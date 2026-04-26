#!/usr/bin/env bash
#MISE description="Check Rust formatting through Bazel"
set -euo pipefail

bazelisk build \
  --aspects=@rules_rust//rust:defs.bzl%rustfmt_aspect \
  --output_groups=rustfmt_checks \
  //...
