# Crook shell integration for fish.
#
# Emits the four OSC 133 marks that tell a terminal where one command ends and
# the next begins: A before the prompt, B where the cursor lands, C when a line
# is accepted for execution, D when it finishes, carrying the exit status.
#
# Crook sources this for you in the shells it starts. Where it cannot reach —
# over ssh, inside a container, on a machine that is not this one — paste it at
# the END of ~/.config/fish/config.fish.

# Three conditions, and the third is the one that matters most here. fish 4.0
# emits all four marks itself, from its own reader, behind the `mark-prompt`
# feature flag; installing these hooks on top of that gives every block two of
# every mark, which is worse than having none. Where fish is already doing the
# job, do nothing — and where a person has turned it off, or fish is older than
# the flag, do it for them.
#
# The second condition's guard is deliberately `set --global` and not
# `set --export`: sourcing this twice in one shell must not install the hooks
# twice, but `exec fish` replaces the shell with one that has no hooks at all,
# and an exported guard would tell that new shell the work was already done.
status is-interactive
and not set --query CROOK_SHELL_INTEGRATION
and not status features | string match --quiet --regex '^mark-prompt\s+on\b'
and begin
    set --global CROOK_SHELL_INTEGRATION 1

    function __crook_mark
        printf '\e]133;%s\a' $argv[1]
    end

    function __crook_preexec --on-event fish_preexec
        __crook_mark C
    end

    function __crook_postexec --on-event fish_postexec
        # $status first, before anything else in this function: a test, a
        # printf, anything at all here replaces the status of the command the
        # user ran.
        set --local exit_status $status
        __crook_mark "D;$exit_status"
    end

    # ctrl-c on a line that was typed but never run. A boundary with no command
    # behind it, and so no status to report — the standard spells that as a
    # bare D. fish is the only one of the three shells that reports it.
    function __crook_cancel --on-event fish_cancel
        __crook_mark D
    end

    function __crook_has_mode_prompt --description 'Whether fish_mode_prompt prints anything'
        functions --query fish_mode_prompt
        and functions fish_mode_prompt | string match --regex --invert --quiet '^ *(#|function |end$|$)'
    end

    # Deferred to the first prompt rather than done here. Crook installs this
    # through vendor_conf.d, which fish reads before config.fish, so at this
    # point fish_prompt is still fish's own and wrapping it would wrap the one
    # the user is about to replace. By the time the fish_prompt event fires,
    # config.fish has run and fish_prompt is theirs.
    function __crook_install_prompt --on-event fish_prompt
        functions --erase __crook_install_prompt
        functions --query fish_prompt
        or return
        functions --copy fish_prompt __crook_inner_prompt
        if __crook_has_mode_prompt
            # fish_mode_prompt prints before fish_prompt, so A belongs at the
            # top of it or the vi-mode indicator lands outside the block.
            functions --copy fish_mode_prompt __crook_inner_mode_prompt
            function fish_mode_prompt
                __crook_mark A
                __crook_inner_mode_prompt
            end
            function fish_prompt
                __crook_inner_prompt
                __crook_mark B
            end
        else
            function fish_prompt
                __crook_mark A
                __crook_inner_prompt
                __crook_mark B
            end
        end
    end
end
