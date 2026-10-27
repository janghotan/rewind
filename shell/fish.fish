# rewind — universal undo for destructive shell commands (fish)
#
# Recommended install (no root, survives upgrades):
#   echo 'rewind init fish | source' >> ~/.config/fish/config.fish
#
# Or source this file:
#   source /usr/share/rewind/fish.fish
#
# Bypass once with:   REWIND_DISABLE=1 rm -rf build/
# Bypass always for a raw binary:  command rm ...   or   /bin/rm ...

if command -q rewind
    function __rewind_run
        set -l __rewind_cmd $argv[1]
        set -l __rewind_args $argv[2..-1]
        if test "$REWIND_DISABLE" = "1"
            command $__rewind_cmd $__rewind_args
        else
            rewind exec $__rewind_cmd -- $__rewind_args
        end
    end

    function rm;    __rewind_run rm $argv;    end
    function mv;    __rewind_run mv $argv;    end
    function cp;    __rewind_run cp $argv;    end
    function chmod; __rewind_run chmod $argv; end
    function chown; __rewind_run chown $argv; end

    function git
        switch $argv[1]
            case reset clean checkout restore branch
                __rewind_run git $argv
            case '*'
                command git $argv
        end
    end

    set -gx REWIND_ACTIVE 1
end
