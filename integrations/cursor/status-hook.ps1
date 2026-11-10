# Cursor Agent CLI hooks for Herdr on Windows: status-hook.sh in PowerShell.
# Reports Cursor's state as an OSC 7501 root record through `hc report`, so the
# app lists a Cursor pane as an agent (working / done / idle). Cursor prints no
# 7501 itself, and it runs hooks detached, with no terminal to print one to,
# hence `hc report` (herdr-core#81).
#
#   status-hook.ps1 start   sessionStart        -> idle
#   status-hook.ps1 prompt  beforeSubmitPrompt  -> working
#   status-hook.ps1 stop    stop                -> done (error: error, aborted: idle)
#   status-hook.ps1 end     sessionEnd          -> the record is cleared
#
# For Windows PowerShell 5.1 and PowerShell 7. Install by hand, see README.md.
# Outside a Herdr terminal (no HC_TERMINAL_ID) it reports nothing. It never
# fails a hook: Cursor gets its answer whatever hc does.
# Env: HERDR_HC (the hc to run, default hc on PATH).
param([string]$Action = "")

# The hook's JSON as UTF-8 whatever the console's code page is.
$inputText = ""
try {
    $reader = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), [Text.Encoding]::UTF8)
    $inputText = $reader.ReadToEnd()
} catch {
}

$body = $null
switch -CaseSensitive ($Action) {
    "start" { $body = "state=idle:app=cursor" }
    "prompt" { $body = "state=working:app=cursor" }
    "stop" {
        $status = $null
        try { $status = ($inputText | ConvertFrom-Json).status } catch {}
        $body = switch -CaseSensitive ("$status") {
            "error" { "state=error:app=cursor" }
            "aborted" { "state=idle:app=cursor" }
            default { "state=done:app=cursor" }
        }
    }
    "end" { $body = "state=clear" }
}

if ($body -and $env:HC_TERMINAL_ID) {
    $hc = if ($env:HERDR_HC) { $env:HERDR_HC } else { "hc" }
    try { & $hc report $body 2>&1 | Out-Null } catch {}
}

# beforeSubmitPrompt must say whether the prompt goes on; the others want an object.
if ($Action -eq "prompt") { Write-Output '{"continue":true}' } else { Write-Output '{}' }
exit 0
