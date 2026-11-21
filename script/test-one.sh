#!/bin/sh
# Focused test run: sh script/test-one.sh [package] [filter]
#
# Runs `cargo test --lib` for one package (or gpui + gpui_apple when no package
# is given) with an optional test-name filter. gpui's test features are added
# only for packages that can take them, and always the same set for the same
# package, so cargo never rebuilds gpui when you switch between packages.
set -u

cd "$(dirname "$0")/.." || exit 1

pkg="${1-}"
filter="${2-}"

# Which of gpui and gpui_apple can `--features` name for package $1? Cargo only
# accepts `dep/feature` for the package itself or a direct, non-optional
# dependency (normal, dev or build) under that exact key, so a transitive
# dependency or a renamed one (`gpui_fast = { package = "gpui" }`) is refused
# and gets no features. Prints one name per line.
gpui_deps() {
    cargo metadata --offline --format-version 1 --no-deps 2>/dev/null |
        python3 -I -c '
import json, sys
pkg = sys.argv[1]
for p in json.load(sys.stdin)["packages"]:
    if p["name"] != pkg:
        continue
    if pkg in ("gpui", "gpui_apple"):
        print(pkg)
    for d in p["dependencies"]:
        key = d.get("rename") or d["name"]
        if key in ("gpui", "gpui_apple") and not d["optional"]:
            print(key)
' "$1"
}

# Is gpui_apple anywhere in $1's normal or dev dependency tree? `cargo tree -i`
# lists $1 itself when it is.
reaches_apple() {
    cargo tree --offline -p "$1" -e normal,dev -i gpui_apple --prefix none 2>/dev/null |
        grep -q "^$1 v"
}

if [ -z "$pkg" ]; then
    pkgs="-p gpui -p gpui_apple"
    features="gpui/test-support,gpui_apple/runtime_shaders"
else
    pkgs="-p $pkg"
    features=""
    if [ "$pkg" = gpui_apple ] || reaches_apple "$pkg"; then
        # Anything that builds gpui_apple, directly or not, needs its
        # runtime_shaders feature (no Metal toolchain to precompile shaders).
        # Cargo only takes `dep/feature` for selected packages' direct
        # dependencies, so select gpui_apple too: that also gives every such
        # package the very same feature set as the no-package run below.
        [ "$pkg" = gpui_apple ] || pkgs="$pkgs -p gpui_apple"
        features="gpui/test-support,gpui_apple/runtime_shaders"
    elif gpui_deps "$pkg" | grep -qx gpui; then
        features="gpui/test-support"
    fi
fi

run() {
    # $1: extra cargo test flags ("--lib" or empty)
    # shellcheck disable=SC2086
    set -- $1 $pkgs ${features:+--features "$features"} ${filter:+"$filter"}
    echo "+ cargo test $*" >&2
    cargo test "$@"
}

log="$(mktemp "${TMPDIR:-/tmp}/test-one.XXXXXX")" || exit 1
code="$log.code"
trap 'rm -f "$log" "$code"' EXIT

# POSIX sh has no pipefail, so cargo's exit code goes through a file.
{ run --lib 2>&1; echo $? >"$code"; } | tee "$log"
status="$(cat "$code")"
if [ "$status" != 0 ] && grep -q "no library targets found" "$log"; then
    echo "no lib target in ${pkg:-gpui}; falling back to a plain run" >&2
    run ""
    exit $?
fi
exit "$status"
