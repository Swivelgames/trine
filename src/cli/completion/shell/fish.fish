function _trine_complete_@IDENTIFIER@
	set -l words (commandline --current-process --tokenize)
	set -l index (count (commandline --current-process --tokenize --cut-at-cursor))
	set -l slot (math $index + 1)
	set -l token (commandline --current-token --cut-at-cursor)
	set -l current (string unescape -- "$token")
	or begin
		# At the cursor a final backslash can still be a literal path separator.
		set current (string unescape -- "$token\\")
		or return
	end

	set words[$slot] "$current"
	env TRINE_COMPLETE=fish _CLAP_COMPLETE_INDEX=$index '@PROGRAM@' -- $words 2>/dev/null
end
complete --keep-order --exclusive --command '@PROGRAM@' --arguments '(_trine_complete_@IDENTIFIER@)'
