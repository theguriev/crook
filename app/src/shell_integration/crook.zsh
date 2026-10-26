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

# `crook`, for the commands Crook's documentation gives, once your own files
# have had their say about PATH: Crook sources this after your .zshrc. The
# shell started with the directory at the end of PATH already, unless it is a
# `bin` in your home, which is your own files' to place; this puts it back
# after a file that assigned PATH outright. Only when no `crook` answers at
# all, and at the end: a crook of your own is yours to run, and $CROOK_BIN is
# the pane's either way. Not when the directory holds a `:`, which a PATH
# would read as two directories. Above the guard, because a copy of this file
# pasted into ~/.zshrc before these lines existed sets it and does none of
# this; running twice is harmless, since once `crook` answers this adds
# nothing.
if [[ ${CROOK_BIN-} == /?*/* && ${CROOK_BIN:h} != *:* && -x $CROOK_BIN ]] && ! whence -p crook >/dev/null; then
	PATH=${PATH:+$PATH:}${CROOK_BIN:h}
fi

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
# The authority is the machine's name — `file://host/path` — because the path
# means a directory on this machine and nowhere else: pasted on the far side of
# an ssh, this is what tells Crook the shell is there, and that the laptop's
# checkout at the same path is not the one it is in. It is printed as it is: a
# machine's name holds none of the bytes escaped below.
#
# Every byte of the path but a letter, a digit and `/._~-` leaves as its `%XX`
# code, which the reader turns back. Not only `%`, which the reader would take
# for the start of an escape, and `;`, which the terminal splits the OSC into
# fields on: a directory name can hold any byte but `/`, and a BEL or an ESC
# printed as itself ends the report there. A clone of a repository that has a
# directory named `x<BEL><ESC>[3J<ESC>c` cut the path short at `x` — the
# directory beside it, which can be one somebody chose — and the rest arrived
# as output at every prompt: the scrollback erased, the terminal reset, mouse
# reporting turned on. The set is the one fish's `string escape --style=url`
# keeps, so the three shells say the same path the same way.
#
# A path of those bytes alone, which is nearly every path, is printed as it
# is. Any other is split into its bytes and each byte outside the set is
# replaced by its code, in one expansion of the whole array, which costs the
# path's length and not the square of it. Bytes, whatever the locale, with
# `nomultibyte`: `(s::)` splits into them and `#MATCH` numbers one. Not with
# a `local LC_ALL=C`, which a startup file that made `LC_ALL` read-only turns
# into an error that ends the function at every prompt. `emulate -L zsh` for
# the options the expansion reads, and `extendedglob` for `(#m)`, both for
# this function alone. The `/` in the pattern is quoted, or it would end it.
__crook_cwd() {
	emulate -L zsh
	local d=$PWD
	if [[ $d == *[^abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789/._~-]* ]]; then
		setopt extendedglob nomultibyte
		local -a bytes
		bytes=( ${(s::)PWD} )
		d=${(j::)bytes/(#m)[^abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789\/._~-]/%${(l:2::0:)$(( [##16] #MATCH & 255 ))}}
	fi
	builtin printf '\e]7;file://%s%s\a' "${__crook_host-}" "$d"
}

# The name is the system's, read once here, and not `$HOST`. zsh takes `$HOST`
# from the environment when it is there, and a startup file can set it to
# anything: conda's compiler packages export a target triple as `HOST`, and a
# dev server is started with `HOST=0.0.0.0`. Either would make a shell on this
# machine look like one somewhere else, and cost its row every git fact. On
# Linux the name is a file the shell reads itself; elsewhere it is one `uname`
# as the shell starts, and no prompt pays for it.
__crook_host=''
if [[ -r /proc/sys/kernel/hostname ]]; then
	builtin read -r __crook_host </proc/sys/kernel/hostname
fi
[[ -n $__crook_host ]] || __crook_host=$(command uname -n 2>/dev/null)
[[ -n $__crook_host ]] || __crook_host=${HOST-}

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

	# The word is the run at the end of the line that no unescaped space
	# breaks: a space with a backslash before it is part of the name, which is
	# how an answer from here writes one, and cutting there made the next Tab
	# ask about the fragment after it. The same rule Crook's `word_at_end`
	# follows, so the word the answer replaces is the word it was for.
	setopt localoptions extendedglob
	local word=${(M)line%%(\\?|[^[:space:]\\])#}
	# Quoted, so the word is taken off the line as itself and not as a glob:
	# `ls *.txt` makes the word `*.txt`, and an unquoted removal would match a
	# different suffix. The globbing this line does not want is where the file
	# candidates below opt into it, with an explicit `${~word}`.
	local prefix=${line%"$word"}
	local -a candidates=()

	# The word is text in the two hash lookups below, not a pattern: `(b)`
	# quotes whatever in it a pattern would read. Unquoted, a `(` or a `[`
	# typed at the start of a line was a pattern zsh could not parse, and what
	# came back was the bodies of this shell's functions, line by line, offered
	# as completions to put on the command line.
	#
	# A `$` is looked at first, because `$EDITOR` is as much a word at the
	# start of a line as anywhere after it, and was completed as a command.
	if [[ $word == \$* ]]; then
		candidates=(\$${^${(k)parameters[(I)${(b)word#\$}*]}})
	elif [[ -z ${prefix//[[:space:]]/} ]]; then
		# Commands: the same hash, functions, builtins and PATH zsh completes
		# from. `(k)` takes the keys of the hash rather than its paths.
		candidates=(${(k)commands[(I)${(b)word}*]} ${(k)functions[(I)${(b)word}*]} ${(k)builtins[(I)${(b)word}*]})
	else
		# The files and directories the word could name. `(N)` makes a glob
		# that matches nothing expand to nothing rather than to itself, and
		# `-M` marks a directory, or a link to one, with a slash — which is
		# what makes an inserted completion carry on being completable. (This
		# said `-/` did that once; `/` only picks directories out, and every
		# directory came back without its slash.)
		#
		# What comes back is put on the line as it is, so it is written the
		# way zsh's own Tab writes it: quoted with `(q)`, or `notes file.txt`
		# is two arguments and `it's.txt` opens a string. A leading `~` or
		# `~user` is taken off before quoting and put back after — the glob
		# expands it, and `~/pro` came back as the whole path it stands for.
		#
		# A word that holds a backslash is a name written out, the way these
		# answers write one, and is looked for as exactly the name it spells:
		# `(Q)` takes the escapes off and `(b)` makes what is left literal. A
		# backslash in a glob that came out of a variable escapes nothing, so
		# `with\ sp` matched no file at all. A word without one is still a
		# glob, so `*.tx` goes on finding `notes.txt`.
		local home='' expanded='' rest pattern found
		if [[ $word == \~* ]]; then
			home=${word%%/*}
			expanded=${~home}
		fi
		rest=${word#$home}
		if [[ $rest == *\\* ]]; then
			pattern=${(b)${(Q)rest}}
		else
			pattern=$rest
		fi
		[[ -n $home ]] && pattern=${(b)expanded}$pattern
		for found in ${~pattern}*(N-M); do
			if [[ -n $home && $found == $expanded* ]]; then
				candidates+=("$home${(q)${found#$expanded}}")
			else
				candidates+=("${(q)found}")
			fi
		done
	fi

	# Sorted and de-duplicated: the caller shows this list to a person, and
	# the three hashes the commands come from can name one command twice.
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
