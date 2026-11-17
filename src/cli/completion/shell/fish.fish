function _trine_complete_@IDENTIFIER@
	set -l words (commandline --current-process --tokenize)
	set -l index (count (commandline --current-process --tokenize --cut-at-cursor))
	set -l slot (math $index + 1)
	set -l current (commandline --current-token --cut-at-cursor)
	set words[$slot] "$current"
	env TRINE_COMPLETE=fish _CLAP_COMPLETE_INDEX=$index '@PROGRAM@' -- $words 2>/dev/null
end
complete --keep-order --exclusive --command '@PROGRAM@' --arguments '(_trine_complete_@IDENTIFIER@)'
