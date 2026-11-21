Register-ArgumentCompleter -Native -CommandName '@PROGRAM@' -ScriptBlock {
	param($wordToComplete, $commandAst, $cursorPosition)

	# Only literal AST values are data. Never evaluate the partially typed command.
	$literal = {
		param($element)
		if ($element -is [System.Management.Automation.Language.StringConstantExpressionAst]) {
			return $element.Value
		}
		if ($element -is [System.Management.Automation.Language.CommandParameterAst] -and $null -eq $element.Argument) {
			return $element.Extent.Text
		}
		return $null
	}
	$words = [System.Collections.Generic.List[string]]::new()
	$index = -1
	foreach ($element in $commandAst.CommandElements) {
		if ($index -lt 0 -and $cursorPosition -lt $element.Extent.StartOffset) {
			$index = $words.Count
			$words.Add('')
		}
		$current = $element
		if ($cursorPosition -ge $element.Extent.StartOffset -and $cursorPosition -le $element.Extent.EndOffset) {
			$index = $words.Count
			$fragment = $element.Extent.Text.Substring(0, $cursorPosition - $element.Extent.StartOffset)
			$tokens = $null
			$errors = $null
			$parsed = [System.Management.Automation.Language.Parser]::ParseInput("x $fragment", [ref]$tokens, [ref]$errors)
			if ($null -eq $parsed.EndBlock -or $parsed.EndBlock.Statements.Count -ne 1) { return '' }
			$statement = $parsed.EndBlock.Statements[0]
			if ($statement -isnot [System.Management.Automation.Language.PipelineAst] -or $statement.PipelineElements.Count -ne 1) { return '' }
			$command = $statement.PipelineElements[0]
			if ($command -isnot [System.Management.Automation.Language.CommandAst]) { return '' }
			if ($fragment.Length -eq 0) {
				$words.Add('')
				continue
			}
			if ($command.CommandElements.Count -ne 2) { return '' }
			$current = $command.CommandElements[1]
		}
		$value = & $literal $current
		if ($null -eq $value) { return '' }
		$words.Add($value)
	}
	if ($index -lt 0) {
		$index = $words.Count
		$words.Add('')
	}

	$previousShell = $env:TRINE_COMPLETE
	$previousMetadata = $env:TRINE_COMPLETE_METADATA
	$previousIndex = $env:_CLAP_COMPLETE_INDEX
	try {
		$env:TRINE_COMPLETE = 'powershell'
		$env:TRINE_COMPLETE_METADATA = '1'
		$env:_CLAP_COMPLETE_INDEX = [string]$index
		$results = @(& '@PROGRAM@' -- @words 2>$null)
	} finally {
		$env:TRINE_COMPLETE = $previousShell
		$env:TRINE_COMPLETE_METADATA = $previousMetadata
		$env:_CLAP_COMPLETE_INDEX = $previousIndex
	}
	if ($results.Count -eq 0) { return '' }
	foreach ($row in $results) {
		$fields = $row -split "`t", 2
		$value = $fields[0]
		$tooltip = if ($fields.Count -gt 1) { $fields[1] } else { $value }
		$quoted = "'" + $value.Replace("'", "''") + "'"
		[System.Management.Automation.CompletionResult]::new($quoted, $value, 'ParameterValue', $tooltip)
	}
}
