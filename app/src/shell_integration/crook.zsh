# Crook shell integration for zsh.
#
# Emits the four OSC 133 marks that tell a terminal where one command ends and
# the next begins: A before the prompt, B where the cursor lands, C when a line
# is accepted for execution, D when it finishes, carrying the exit status.
#
# Crook sources this for you in the shells it starts. Where it cannot reach —
# over ssh, inside a container, on a machine that is not this one — paste it at
# the END of ~/.zshrc, after whatever sets your prompt.

# A script has no prompt to mark, and no user waiting at one.
[[ -o interactive ]] || return 0

# Deliberately not exported. Sourcing this twice in one shell must not install
# the hooks twice; `exec zsh` replaces the shell with one that has no hooks at
# all, and an exported guard would tell that new shell the work was already
# done.
if [[ -n ${CROOK_SHELL_INTEGRATION-} ]]; then
	return 0
fi
CROOK_SHELL_INTEGRATION=1

__crook_mark() { builtin printf '\e]133;%s\a' "$1" }

# Where the shell is, reported the way every terminal reads it: OSC 7.
#
# Nothing else tells Crook. A tab records a directory when it is made and would
# keep printing that one forever — after any `cd`, and after `Crook.app` is
# opened from the Dock, where macOS hands a GUI process `/` as its working
# directory and the tab would say so while the shell sat somewhere else.
#
# The authority is left empty — `file:///path`, not `file://host/path` —
# because the host is ignored at the other end and asking for it would mean a
# `hostname` call on every prompt.
#
# Only `%` is escaped. It is the one byte the reader could mistake for the
# start of an escape it should decode; everything else survives the round trip
# as itself, spaces included.
__crook_cwd() { builtin printf '\e]7;file://%s\a' "${PWD//\%/%25}" }

# Completion, which is the one thing the marks cannot do: they are an
# announcement, and this is a question with an answer.
#
# The line does not arrive in the key sequence. It is in a file Crook wrote — a
# command line can hold a semicolon, a newline and bytes that are not UTF-8, and
# escaping all of them past a shell and past an OSC parser twice over is a
# protocol nobody should have to debug. The answer goes back the same way, and
# the escape sequence carries only the request's number.
#
# **This is the weakest of the three, and the reason is zsh's.** Its completion
# system is a ZLE thing: `_main_complete` runs inside a widget, against the
# widget's own buffer, and reports through `compstate` rather than returning
# anything. There is no `compgen` to ask and no `complete -C` to ask with, so
# what this offers is commands, files and variables — the answers zsh's own
# expansion gives — and not the `_git`, `_docker` and `_ssh` definitions that
# make zsh's completion what it is. It is written down here rather than
# discovered.
__crook_complete() {
	[[ -n ${CROOK_SCRATCH-} ]] || return 0
	local request="$CROOK_SCRATCH/complete.in"
	local answer="$CROOK_SCRATCH/complete.out"
	[[ -f $request ]] || return 0

	local content serial line
	content=$(<"$request")
	serial=${content%%$'\n'*}
	# The rest is the line up to the caret, newlines and all.
	line=${content#*$'\n'}
	[[ $line == "$content" ]] && line=''

	local word=${line##* }
	local prefix=${line%$word}
	local -a candidates=()

	if [[ -z ${prefix//[[:space:]]/} ]]; then
		# Commands: the same hash, functions, builtins and PATH zsh completes
		# from. `(k)` takes the keys of the hash rather than its paths.
		candidates=(${(k)commands[(I)$word*]} ${(k)functions[(I)$word*]} ${(k)builtins[(I)$word*]})
	elif [[ $word == \$* ]]; then
		candidates=(\$${^${(k)parameters[(I)${word#\$}*]}})
	else
		# `(N)` makes a glob that matches nothing expand to nothing rather than
		# to itself, and `-/` puts a slash on a directory — which is what makes
		# an inserted completion carry on being completable.
		candidates=(${~word}*(N) ${~word}*(N-/))
	fi

	# Sorted and de-duplicated: the two globs above overlap on directories, and
	# the caller shows this list to a person.
	builtin printf '%s\n' ${(ou)candidates} >"$answer"
	builtin printf '\e]6339;%s\a' "$serial"
}

# A widget rather than a `bindkey -s`, because the work is a function and the
# line editor must not be handed anything to insert. `zle -R` is not needed:
# nothing here prints where the person can see it.
zle -N __crook_complete
# Every keymap a person could be typing in: `main` is whichever of emacs and
# viins is in force, and `viins` is named as well for a shell that switches
# after this runs.
bindkey '\e[6339~' __crook_complete
bindkey -M viins '\e[6339~' __crook_complete 2>/dev/null

# %{...%} is how zsh is told a stretch of prompt occupies no columns. Without
# it the shell miscounts the prompt width and every long line the user types
# wraps in the wrong place — which reads as a terminal bug, not a shell one.
__crook_prompt_start=$'%{\e]133;A\a%}'
__crook_prompt_end=$'%{\e]133;B\a%}'

# A and B live inside PS1 rather than being printed from a hook, because zle
# redraws the prompt on every reflow, completion and syntax-highlighting pass,
# and only what is part of the prompt string is redrawn with it.
#
# Re-checked before every prompt: powerlevel10k, starship and transient prompts
# assign PS1 from their own precmd hook, so a wrapper applied once at startup
# is gone by the second prompt.
__crook_marked_prompt=''
__crook_mark_prompt() {
	[[ -n $__crook_marked_prompt && $PS1 == "$__crook_marked_prompt" ]] && return
	PS1=$__crook_prompt_start$PS1$__crook_prompt_end
	__crook_marked_prompt=$PS1
}

# Whether a command actually ran since the last prompt, and whether a prompt
# has ever been drawn. Together they keep the first prompt of the session from
# reporting the completion of a command nobody typed.
__crook_ran=''
__crook_prompted=''

__crook_preexec() {
	__crook_ran=1
	__crook_mark C
}

__crook_precmd() {
	# $? first, before anything else in this function. A test, a printf, an
	# assignment — anything at all here replaces the status of the command the
	# user ran, and every command then reports success.
	local exit_status=$?
	if [[ -n $__crook_ran ]]; then
		__crook_mark "D;$exit_status"
	elif [[ -n $__crook_prompted ]]; then
		# An empty line, or one abandoned with ctrl-c: a boundary with no
		# command behind it, and so no status to report. The standard spells
		# that as a bare D.
		__crook_mark D
	fi
	__crook_ran=''
	__crook_prompted=1
	__crook_cwd
}

# Prepended rather than appended, and filtered first so a second source cannot
# double it: this is the hook that reads $?, so nothing the user registered may
# run before it. Assigning the arrays directly rather than through add-zsh-hook
# is what makes "first" expressible at all.
precmd_functions=(__crook_precmd ${precmd_functions:#__crook_precmd})
preexec_functions=(__crook_preexec ${preexec_functions:#__crook_preexec})

# The prompt wrapper is the opposite case: it must run last, after every hook
# that might have assigned PS1 for this prompt.
precmd_functions=(${precmd_functions:#__crook_mark_prompt} __crook_mark_prompt)
