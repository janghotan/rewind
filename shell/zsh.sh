# rewind — universal undo for destructive shell commands (zsh)
#
# Recommended install (no root, survives upgrades):
#   echo 'eval "$(rewind init zsh)"' >> ~/.zshrc
#
# Or source this file:
#   source /usr/share/rewind/zsh.sh
#
# Bypass once with:   REWIND_DISABLE=1 rm -rf build/
# Bypass always for a raw binary:  command rm ...   or   /bin/rm ...

if (( $+commands[rewind] )); then
    __rewind_run() {
        local __rewind_cmd=$1
        shift
        if [[ "${REWIND_DISABLE:-0}" == "1" ]]; then
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

    git() {
        case "$1" in
            reset|clean|checkout|restore) __rewind_run git "$@" ;;
            *) command git "$@" ;;
        esac
    }

    export REWIND_ACTIVE=1
fi
