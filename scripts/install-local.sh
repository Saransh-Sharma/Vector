#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"

cargo install --locked --path "$repo_dir/crates/vector-cli"
cargo install --locked --path "$repo_dir/crates/vectord"
cargo_bin_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
ln -sf vector-agent "$cargo_bin_dir/vctr"

echo "Installed vector-agent, vctr, and vectord."
echo "Start with: vctr init"
