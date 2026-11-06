#!/usr/bin/env bash
set -euo pipefail

interpreter="${1:-pwsh}"
command -v "$interpreter" >/dev/null
# PowerShell owns these variables; Bash must pass the script literally.
# shellcheck disable=SC2016
"$interpreter" -NoProfile -NonInteractive -Command '
$ErrorActionPreference = "Stop"
$PSVersionTable.PSVersion.ToString()
[System.Collections.Specialized.NameValueCollection].FullName
[Console]::WriteLine("powershell-runtime-ok")
'
