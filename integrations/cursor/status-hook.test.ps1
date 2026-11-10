# powershell -NoProfile -ExecutionPolicy Bypass -File integrations\cursor\status-hook.test.ps1
# (or pwsh -NoProfile -File ...). Runs status-hook.ps1 the way Cursor does, as a
# child PowerShell fed the hook's JSON, with a fake hc that records what it was
# asked to report.
$ErrorActionPreference = "Stop"
$hook = Join-Path $PSScriptRoot "status-hook.ps1"
$shell = (Get-Process -Id $PID).Path
$tmp = Join-Path ([IO.Path]::GetTempPath()) "cursor-status-hook-test-$PID"
New-Item -ItemType Directory -Force $tmp | Out-Null
$log = Join-Path $tmp "log"
$fake = Join-Path $tmp "hc.ps1"
$env:FAKE_LOG = $log
$env:HERDR_HC = $fake
$recording = 'Add-Content -LiteralPath $env:FAKE_LOG -Value ($args -join " ")'
Set-Content -LiteralPath $fake -Value $recording
$script:fails = 0

# Check <name> <action> <stdin> <expected hc args, or - for none> <expected stdout>
function Check($name, $action, $stdin, $want, $wantOut) {
    [IO.File]::WriteAllText($log, "")
    $out = $stdin | & $shell -NoProfile -ExecutionPolicy Bypass -File $hook $action
    $code = $LASTEXITCODE
    $got = ([IO.File]::ReadAllText($log)).Trim()
    if (-not $got) { $got = "-" }
    $out = (@($out) -join "`n").Trim()
    if ($got -ne $want -or $out -ne $wantOut -or $code -ne 0) {
        Write-Output "FAIL ${name}: hc '$got' (want '$want'), stdout '$out' (want '$wantOut'), exit $code"
        $script:fails++
    } else {
        Write-Output "ok   $name"
    }
}

try {
    $env:HC_TERMINAL_ID = "t7"
    Check "session start: an idle agent"     start  '{"session_id":"s"}'     "report state=idle:app=cursor"    '{}'
    Check "prompt: working, and lets it go"  prompt '{"prompt":"hi"}'        "report state=working:app=cursor" '{"continue":true}'
    Check "stop completed: done"             stop   '{"status":"completed"}' "report state=done:app=cursor"    '{}'
    Check "stop error: error"                stop   '{"status":"error"}'     "report state=error:app=cursor"   '{}'
    Check "stop aborted: idle"               stop   '{"status":"aborted"}'   "report state=idle:app=cursor"    '{}'
    Check "stop without a status: done"      stop   '{}'                     "report state=done:app=cursor"    '{}'
    Check "stop with junk input: done"       stop   'not json'               "report state=done:app=cursor"    '{}'
    Check "stop with a UTF-8 prompt: error"  stop   '{"status":"error","text":"héllo ✓"}' "report state=error:app=cursor" '{}'
    Check "session end: clears"              end    '{"reason":"completed"}' "report state=clear"              '{}'
    Check "unknown event: nothing"           what   '{}'                     "-"                               '{}'

    # A failing or missing hc must never block Cursor: the prompt still goes through.
    Set-Content -LiteralPath $fake -Value 'exit 1'
    Check "hc fails: prompt still continues" prompt '{}'                     "-"                               '{"continue":true}'
    $env:HERDR_HC = Join-Path $tmp "no-such-hc.exe"
    Check "no hc: prompt still continues"    prompt '{}'                     "-"                               '{"continue":true}'

    $env:HERDR_HC = $fake
    Set-Content -LiteralPath $fake -Value $recording
    Remove-Item Env:HC_TERMINAL_ID
    Check "outside Herdr: nothing reported"  prompt '{}'                     "-"                               '{"continue":true}'
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

if ($script:fails -eq 0) { Write-Output "all passed"; exit 0 }
Write-Output "$($script:fails) failed"
exit 1
