# rewind — universal undo for destructive shell commands (bash)
#
# Recommended install (no root, survives upgrades):
#   echo 'eval "$(rewind init bash)"' >> ~/.bashrc
#
# Or source this file:
#   source /usr/share/rewind/bash.sh
#
# Bypass once with:   REWIND_DISABLE=1 rm -rf build/
# Bypass always for a raw binary:  command rm ...   or   /bin/rm ...

if command -v rewind >/dev/null 2>&1; then
    __rewind_run() {
        local __rewind_cmd=$1
        shift
        if [ "${REWIND_DISABLE:-0}" = "1" ]; then
            command "$__rewind_cmd" "$@"
        else
            rewind exec "$__rewind_cmd" -- "$@"
        fi
    }

    rm()    { __rewind_run rm "$@"; }
    mv()    { __rewind_run mv "$@"; }
    cp()    { __rewind_run cp "$@"; }
    chmod() { __rewind_run chmod "$@"; }
    chown() { __rewind_run chown "$@"; }

    # Only route the destructive git subcommands through rewind; every other
    # git invocation goes straight to the real binary.
    git() {
        case "$1" in
            reset|clean|checkout|restore|branch) __rewind_run git "$@" ;;
            *) command git "$@" ;;
        esac
    }

    export REWIND_ACTIVE=1
fi
