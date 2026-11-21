#compdef @PROGRAM@
_trine_complete_@IDENTIFIER@() {
	local -a request=("${(@Q)words}")
	# Zsh removes the surrounding quotes from PREFIX. Restore that context so
	# unquoting retains literal backslashes inside a quoted filesystem path.
	local current="$PREFIX" quote="$compstate[quote]"
	if [[ "$quote" == "'" || "$quote" == '"' ]]; then
		current="${quote}${PREFIX}${quote}"
	fi
	request[CURRENT]="${(Q)current}"

	local group value help
	local -a groups=() candidates=() descriptions=()
	while IFS= read -r group && IFS= read -r value && IFS= read -r help; do
		groups+=("$group")
		candidates+=("$value")
		descriptions+=("$help")
	done < <(
		TRINE_COMPLETE=zsh TRINE_COMPLETE_METADATA=1 _CLAP_COMPLETE_INDEX=$((CURRENT - 1)) \
			'@PROGRAM@' -- "${request[@]}" 2>/dev/null
	)

	local index display heading configured group_index hidden
	local -a values directories displays directory_displays expl
	for group in "${(@u)groups}"; do
		values=() directories=() displays=() directory_displays=()
		for (( index = 1; index <= ${#candidates}; index++ )); do
			[[ "${groups[index]}" != "$group" ]] && continue
			value="${candidates[index]}"
			display="$value"
			[[ -n "${descriptions[index]}" ]] && display+=" -- ${descriptions[index]}"

			if [[ "$value" == */ || ( @WINDOWS_PATHS@ == true && "$value" == *'\' ) ]]; then
				directories+=("$value")
				directory_displays+=("$display")
			else
				values+=("$value")
				displays+=("$display")
			fi
		done

		# Let completion styles own formatting; keep headings visible by default.
		# compadd -X interprets percent escapes, unlike literal insertion/display arrays.
		heading="${group//\%/%%}"
		_description "$group" expl "$heading"
		hidden=''
		zstyle -s ":completion:${curcontext}:$group" hidden hidden
		if [[ "$hidden" != all ]] && \
			! zstyle -s ":completion:${curcontext}:$group" format configured && \
			! zstyle -s ":completion:${curcontext}:descriptions" format configured; then
			expl+=(-X "$heading")
		fi
		if ! zstyle -s ":completion:${curcontext}:$group" group-name configured; then
			group_index=${expl[(I)-J]}
			expl[$((group_index + 1))]="$group"
		fi

		(( ${#directories} )) && compadd "${expl[@]}" -S '' -d directory_displays -a directories
		(( ${#values} )) && compadd "${expl[@]}" -d displays -a values
	done
	return 0
}

compdef _trine_complete_@IDENTIFIER@ '@PROGRAM@'
