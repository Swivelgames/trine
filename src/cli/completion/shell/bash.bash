# Candidates are words, not necessarily files. Keep domain names exact.
_trine_complete_@IDENTIFIER@() {
	local -a words=("${COMP_WORDS[@]}")
	local candidate quote='' directory=false
	words[COMP_CWORD]="$2"
	COMPREPLY=()

	while IFS= read -r -d $'\013' candidate; do
		if [[ -z "$quote" ]]; then
			quote=$candidate
			continue
		fi

		[[ -z "$candidate" ]] && continue
		[[ "$candidate" == */ ]] && directory=true

		case "$quote" in
			single) candidate=${candidate//\'/\'\\\'\'} ;;
			double)
				candidate=${candidate//\\/\\\\}
				candidate=${candidate//\"/\\\"}
				candidate=${candidate//\$/\\\$}
				candidate=${candidate//\`/\\\`}
				;;
			*) printf -v candidate '%q' "$candidate" ;;
		esac

		COMPREPLY+=("$candidate")
	done < <(
		TRINE_COMPLETE=bash TRINE_COMPLETE_QUOTE=1 TRINE_COMPLETE_WORD="$2" \
			TRINE_COMPLETE_LINE="$COMP_LINE" TRINE_COMPLETE_POINT="$COMP_POINT" \
			_CLAP_COMPLETE_INDEX="$COMP_CWORD" \
			'@PROGRAM@' -- "${words[@]}" 2>/dev/null
		printf '\013'
	)

	# Only explicit directory candidates continue without a space. Bash 3 lacks
	# compopt; its registration keeps continuation enabled for every candidate.
	if type compopt &>/dev/null; then
		if [[ "$directory" == true ]]; then
			compopt -o nospace
		else
			compopt +o nospace
		fi
	fi
}

complete -o nospace -F _trine_complete_@IDENTIFIER@ '@PROGRAM@'
