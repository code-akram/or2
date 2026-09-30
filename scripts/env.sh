#!/usr/bin/env bash
# Source this file before Cargo/Gradle commands. Existing toolchain overrides win.
export JAVA_HOME="${JAVA_HOME:-$HOME/.local/share/jdk/17}"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/.local/share/android}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/android-ndk-r30}"
export PATH="$JAVA_HOME/bin:$ANDROID_HOME/cmdline-tools/15859902/bin:$ANDROID_HOME/platform-tools:$HOME/.local/share/gradle/gradle-8.13/bin:$PATH"
