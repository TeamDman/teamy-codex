param(
	[Parameter(ValueFromRemainingArguments = $true)]
	[string[]]$QueryArgs
)

if (-not $QueryArgs) {
	$QueryArgs = @("--help")
}

$profiler = Get-Command teamy-profiler -ErrorAction SilentlyContinue
if (-not $profiler) {
	throw "teamy-profiler not found in PATH"
}

& $profiler.Source run cargo `
	--project $PSScriptRoot `
	--bin teamy-codex `
	--profile release `
	--feature tracy `
	-- @QueryArgs

exit $LASTEXITCODE
