#Requires -RunAsAdministrator
<#
.SYNOPSIS
    Removes a hostname entry from the hosts file.
.PARAMETER Hostname
    The hostname to remove.
.EXAMPLE
    .\Remove-Hostname.ps1 -Hostname sql.corp.example.com
#>
param(
    [Parameter(Mandatory=$true)]
    [string]$Hostname
)

$hostsPath = "$env:SystemRoot\System32\drivers\etc\hosts"
$content = Get-Content $hostsPath
$filtered = $content | Where-Object { $_ -notmatch "^\s*\S+\s+$([regex]::Escape($Hostname))\s*$" }

if ($content.Count -eq $filtered.Count) {
    Write-Warning "No entry found for '$Hostname'"
} else {
    $filtered | Set-Content $hostsPath
    Write-Host "Removed entry for '$Hostname'"
}
