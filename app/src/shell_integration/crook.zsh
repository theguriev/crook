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
