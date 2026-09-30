#!/bin/sh
set -eu
cd "$(dirname "$0")"
export GHOSTTY_SOURCE_DIR="$PWD/ghostty-source"
cargo run --release
export ANDROID_NDK_HOME=$HOME/.local/share/android/android-ndk-r30
cargo ndk -t arm64-v8a --platform 31 build --release
