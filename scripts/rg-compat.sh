# Sourced by audit scripts that grep with ripgrep. The audits must produce
# the same verdict on a dev machine (ripgrep installed) and on a bare CI
# runner (no ripgrep): when `rg` is absent, shim the exact flag surface the
# audit scripts use onto GNU `grep`. The mapped surface is deliberately
# tiny — `-q/-n/-i/-v/--`, stdin pipes, multi-file matching,
# fixed-string matching, and `--no-filename` (grep's `-h`). A future audit needing an rg-only feature
# must extend this shim rather than assume the binary, or the runner goes
# red for a missing tool instead of a real contract violation (the exact
# failure mode this file removes).
if ! command -v rg >/dev/null 2>&1; then
    rg() {
        local arg matcher='-E' args=()
        for arg in "$@"; do
            if [ "$arg" = "--no-filename" ]; then
                args+=(-h)
            else
                args+=("$arg")
            fi
            case "$arg" in
                -*F*) matcher='' ;;
            esac
        done
        if [ -n "$matcher" ]; then
            grep "$matcher" "${args[@]}"
        else
            grep "${args[@]}"
        fi
    }
fi
