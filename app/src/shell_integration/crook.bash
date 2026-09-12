# Crook shell integration for bash.
#
# Emits the four OSC 133 marks that tell a terminal where one command ends and
# the next begins: A before the prompt, B where the cursor lands, C when a line
# is accepted for execution, D when it finishes, carrying the exit status.
#
# Crook sources this for you in the shells it starts. Where it cannot reach —
# over ssh, inside a container, on a machine that is not this one — paste it at
# the END of ~/.bashrc, after whatever sets your prompt.

# A script has no prompt to mark, and no user waiting at one.
case $- in
	*i*) ;;
	*) return 0 ;;
esac

# Deliberately not exported. Sourcing this twice in one shell must not install
# the hooks twice; `exec bash` replaces the shell with one that has no hooks at
# all, and an exported guard would tell that new shell the work was already
# done.
if [ -n "${CROOK_SHELL_INTEGRATION-}" ]; then
	return 0
fi
CROOK_SHELL_INTEGRATION=1

__crook_mark() { builtin printf '\e]133;%s\a' "$1"; }

# Where the shell is, reported the way every terminal reads it: OSC 7. See the
# zsh integration for why nothing else tells Crook, and why the authority is
# left empty and only `%` escaped.
__crook_cwd() { builtin printf '\e]7;file://%s\a' "${PWD//\%/%25}"; }

# Completion, which is the one thing the marks cannot do: they are an
# announcement, and this is a question with an answer.
#
# The line does not arrive in the key sequence. It is in a file Crook wrote — a
# command line can hold a semicolon, a newline and bytes that are not UTF-8, and
# escaping all of them past a shell and past an OSC parser twice over is a
# protocol nobody should have to debug. The answer goes back the same way, and
# the escape sequence carries only the request's number.
__crook_complete() {
	[ -n "${CROOK_SCRATCH-}" ] || return 0
	local request="$CROOK_SCRATCH/complete.in"
	local answer="$CROOK_SCRATCH/complete.out"
	[ -f "$request" ] || return 0

	local serial line
	{
		IFS= read -r serial
		# The rest of the file is the line up to the caret. `read -d ''` takes
		# it whole, newlines included, and reports failure at end of file even
		# though it read everything — which is why its status is ignored.
		IFS= read -r -d '' line || true
	} <"$request"

	local -a candidates=()
	__crook_candidates "$line"
	builtin printf '%s\n' ${candidates[@]+"${candidates[@]}"} >"$answer"
	builtin printf '\e]6339;%s\a' "$serial"
}

# What `line` could become, into the `candidates` array of the caller.
#
# The word being completed is everything after the last unquoted space, which is
# an approximation and a deliberate one: bash's own splitting is
# `COMP_WORDBREAKS` and a parser nobody wants twice. It is right for every line
# that does not quote a space, which is nearly all of them, and wrong in a way
# that offers too *few* completions rather than the wrong ones.
__crook_candidates() {
	local line=$1
	local word=${line##* }
	local prefix=${line%"$word"}

	# The first word of the line is a command; everything after it is an
	# argument. `compgen -c` reads the same hash and PATH the shell completes
	# from, so this is the shell's answer rather than an imitation of it.
	if [ -z "${prefix//[[:space:]]/}" ]; then
		mapfile -t candidates < <(builtin compgen -c -- "$word" 2>/dev/null)
		return 0
	fi

	case $word in
		# A variable, which `compgen -v` knows and no glob does.
		\$*) mapfile -t candidates < <(builtin compgen -P '$' -v -- "${word#\$}" 2>/dev/null) ;;
		# `-o default` is what makes a directory come back with its slash and a
		# name with a space come back quoted, both of which the caller inserts
		# verbatim.
		*) mapfile -t candidates < <(builtin compgen -o default -- "$word" 2>/dev/null) ;;
	esac
}

# `bind -x` runs the function and then redraws the prompt, which is exactly
# right: nothing is printed but an OSC, and the line the person is typing is
# Crook's rather than readline's, so there is nothing on screen to disturb.
#
# Guarded because a bash built without readline — or one whose stdin is not a
# terminal by the time this runs — has no `bind` at all. And not on bash 3,
# which is the bash macOS ships: its `bind -x` cannot run a command bound to
# a key *sequence* — every press of the sequence prints `cannot find keymap
# for command` into the pane — and `mapfile` above is bash 4's anyway. A
# shell that cannot answer is left unasked; Crook's Tab then does nothing,
# which is what it did before there was a question to ask.
if [ "${BASH_VERSINFO[0]}" -ge 4 ]; then
	builtin bind -x '"\e[6339~": __crook_complete' 2>/dev/null || true
fi

# \[ ... \] is how readline is told a stretch of prompt prints nothing. Without
# it bash miscounts the prompt width and every long line the user types wraps in
# the wrong place — which reads as a terminal bug, not a shell one.
__crook_prompt_start='\[\e]133;A\a\]'
__crook_prompt_end='\[\e]133;B\a\]'

# A and B live inside PS1 rather than being printed from a hook, because
# readline redraws the prompt on every reflow and every history search, and only
# what is part of the prompt string is redrawn with it.
#
# Re-checked before every prompt: starship and its neighbours assign PS1 from
# their own prompt command, so a wrapper applied once at startup is gone by the
# second prompt.
__crook_marked_prompt=''
__crook_mark_prompt() {
	if [ -z "$__crook_marked_prompt" ] || [ "$PS1" != "$__crook_marked_prompt" ]; then
		PS1="$__crook_prompt_start$PS1$__crook_prompt_end"
		__crook_marked_prompt=$PS1
	fi
}

# Starts set, and cleared by the first __crook_prompt_ready: bash runs the DEBUG
# trap for the commands inside PROMPT_COMMAND as well as for the ones the user
# typed, so without a flag that is already up before the first prompt, whatever
# the user had in PROMPT_COMMAND would each look like a command they ran.
__crook_running=1
# Whether a command actually ran since the last prompt, and whether a prompt has
# ever been drawn. Together they keep the first prompt of the session from
# reporting the completion of a command nobody typed.
__crook_ran=''
__crook_prompted=''
# The history number as of the last prompt. See __crook_precmd.
__crook_history=''
# Whatever DEBUG trap was already installed, so ours can call it rather than
# replace it.
__crook_prior_debug=''

__crook_command_started() {
	[ -n "$__crook_running" ] && return 0
	__crook_running=1
	__crook_ran=1
	__crook_mark C
}

__crook_precmd() {
	# $? first, before anything else in this function. A test, a printf, an
	# assignment — anything at all here replaces the status of the command the
	# user ran, and every command then reports success.
	local exit_status=$?
	# This runs first in PROMPT_COMMAND and __crook_prompt_ready runs last, so
	# raising the flag here is what covers the whole chain — including the
	# prompt commands the user already had, which the DEBUG trap would
	# otherwise report as commands they ran on any prompt where they ran none.
	__crook_running=1
	# bash runs no DEBUG trap at all for a top-level `( ... )`, so the absence
	# of a C mark does not mean nothing ran and would cost that line its exit
	# status. The history number does mean it: bash advances it for every line
	# it accepts and leaves it alone for an empty one. Except on bash 3.2, where
	# HISTCMD never moves at all — there, and only there, a subshell line
	# reports a bare D rather than its status.
	local entered=$HISTCMD
	if [ -n "$__crook_ran" ] || \
		{ [ -n "$__crook_prompted" ] && [ "$entered" != "$__crook_history" ]; }; then
		__crook_mark "D;$exit_status"
	elif [ -n "$__crook_prompted" ]; then
		# An empty line, or one abandoned with ctrl-c: a boundary with no
		# command behind it, and so no status to report. The standard spells
		# that as a bare D.
		__crook_mark D
	fi
	__crook_history=$entered
	__crook_ran=''
	__crook_prompted=1
	__crook_cwd
	# Hand the status on unchanged. This runs first in PROMPT_COMMAND, and
	# whatever the user already had there is entitled to the same $? it saw
	# before Crook inserted itself in front of it.
	return $exit_status
}

__crook_prompt_ready() {
	local exit_status=$?
	__crook_running=''
	__crook_mark_prompt
	return $exit_status
}

__crook_debug_trap() {
	local exit_status=$?
	if [ -n "$__crook_prior_debug" ]; then
		builtin eval "$__crook_prior_debug"
	fi
	# Our own prompt commands are commands too, and the first of them runs
	# before the flag that suppresses the rest is up.
	case $BASH_COMMAND in
		__crook_*) ;;
		*) __crook_command_started ;;
	esac
	return $exit_status
}

if [ -n "${bash_preexec_imported:-}${__bp_imported:-}" ]; then
	# bash-preexec already owns the DEBUG trap, and it arrives under a lot of
	# things people install — starship, ble.sh, atuin. A second DEBUG trap
	# breaks both, so use the lists it exists to provide.
	preexec_functions+=(__crook_command_started)
	precmd_functions=(__crook_precmd ${precmd_functions[@]+"${precmd_functions[@]}"} __crook_prompt_ready)
else
	__crook_trap_dump="$(trap -p DEBUG)"
	if [ -n "$__crook_trap_dump" ]; then
		# `trap -p DEBUG` prints the command that would reinstall the trap:
		# trap -- '<shellcode>' DEBUG. Reading that line back as an array is
		# how the shellcode comes out quoted exactly as it went in.
		__crook_trap_parts=()
		builtin eval "__crook_trap_parts=( $__crook_trap_dump )"
		__crook_prior_debug=${__crook_trap_parts[2]}
		unset __crook_trap_parts
	fi
	unset __crook_trap_dump
	trap '__crook_debug_trap' DEBUG

	# bash 5.1 let PROMPT_COMMAND be an array as well as a string, and the array
	# form is the one that silently loses a string appended to it. The version
	# test is not decoration: an older bash runs only element 0 of an array
	# PROMPT_COMMAND, so treating one as an array there installs a chain it will
	# never run, and the shell gets no prompt marks at all.
	__crook_array_prompt_command=''
	if [ "${BASH_VERSINFO[0]}" -gt 5 ] ||
		{ [ "${BASH_VERSINFO[0]}" -eq 5 ] && [ "${BASH_VERSINFO[1]}" -ge 1 ]; }; then
		case $(builtin declare -p PROMPT_COMMAND 2>/dev/null) in
			"declare -a"*) __crook_array_prompt_command=1 ;;
		esac
	fi

	if [ -n "$__crook_array_prompt_command" ]; then
		PROMPT_COMMAND=(__crook_precmd ${PROMPT_COMMAND[@]+"${PROMPT_COMMAND[@]}"} __crook_prompt_ready)
	else
		# Newline-separated rather than `;`, so a prompt command that ends in a
		# comment does not swallow what follows it. First, because __crook_precmd
		# is what reads $?; last, because __crook_prompt_ready is what wraps the
		# PS1 the rest of the chain may just have assigned. On a bash too old for
		# the array form this assignment lands in element 0, which is the only
		# element that bash was going to run.
		__crook_prior_prompt_command=${PROMPT_COMMAND:-}
		PROMPT_COMMAND=$'__crook_precmd\n'
		if [ -n "$__crook_prior_prompt_command" ]; then
			PROMPT_COMMAND=$PROMPT_COMMAND$__crook_prior_prompt_command$'\n'
		fi
		PROMPT_COMMAND=$PROMPT_COMMAND'__crook_prompt_ready'
		unset __crook_prior_prompt_command
	fi
	unset __crook_array_prompt_command
fi
