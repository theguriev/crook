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

    # Completion, which is the one thing the marks above cannot do: they are an
    # announcement, and this is a question with an answer.
    #
    # The line does not arrive in the key sequence. It is in a file Crook wrote
    # — a command line can hold a semicolon, a newline and bytes that are not
    # UTF-8, and escaping all of them past a shell and past an OSC parser twice
    # over is a protocol nobody should have to debug. The answer goes back the
    # same way, and the escape sequence carries only the request's number.
    #
    # fish is the shell this is easy in: `complete -C` is exactly this
    # question, and its answer is the real one — every `complete` definition
    # fish has, not an approximation built out of globs.
    function __crook_complete
        test -n "$CROOK_SCRATCH"; or return
        set --local request "$CROOK_SCRATCH/complete.in"
        set --local answer "$CROOK_SCRATCH/complete.out"
        test -f "$request"; or return

        # The first line is the request's number; everything after it is the
        # line up to the caret. It is the *prefix* rather than the whole line
        # and a cursor offset, because that is the question every shell's
        # completion actually answers.
        set --local content (cat "$request")
        set --local serial $content[1]
        set --local line (string join \n $content[2..-1])

        # `complete --do-complete` answers with `completion<TAB>description`,
        # and each completion is the whole word rather than the part that is
        # missing — which is what Crook wants, because it is replacing a word.
        complete --do-complete="$line" 2>/dev/null \
            | string replace --regex '\t.*$' '' >"$answer"
        printf '\e]6339;%s\a' $serial
    end

    bind \e\[6339~ __crook_complete
    # fish keeps insert and default mode bindings apart, and a person in vi
    # mode types in insert mode.
    bind --mode insert \e\[6339~ __crook_complete 2>/dev/null

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
