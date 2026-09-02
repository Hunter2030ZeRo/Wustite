#!/bin/sh

set -eu

repository=${WUSTITE_REPOSITORY:-https://github.com/Hunter2030ZeRo/Wustite}
install_root=${WUSTITE_INSTALL_ROOT:-${CARGO_INSTALL_ROOT:-}}

if ! command -v cargo >/dev/null 2>&1; then
    echo "error: Cargo is required. Install Rust from https://rustup.rs/ and try again." >&2
    exit 1
fi

set -- install --git "$repository" --locked --force
if [ -n "$install_root" ]; then
    set -- "$@" --root "$install_root"
fi
set -- "$@" wustite

echo "Installing Wustite from $repository"
cargo "$@"

if [ -n "$install_root" ]; then
    bin_dir=$install_root/bin
elif [ -n "${CARGO_HOME:-}" ]; then
    bin_dir=$CARGO_HOME/bin
else
    bin_dir=$HOME/.cargo/bin
fi

echo "Wustite installed at $bin_dir/wustite"
case ":${PATH:-}:" in
    *":$bin_dir:"*) ;;
    *) echo "Add $bin_dir to PATH to run wustite from any directory." ;;
esac
