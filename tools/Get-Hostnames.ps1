<#
.SYNOPSIS
    Lists all custom entries in the hosts file.
#>
$hostsPath = "$env:SystemRoot\System32\drivers\etc\hosts"
Get-Content $hostsPath | Where-Object { $_ -match "^\s*\d" -and $_ -notmatch "^\s*#" } | ForEach-Object {
    $parts = $_ -split '\s+', 2
    [PSCustomObject]@{
        IPAddress = $parts[0]
        Hostname  = $parts[1].Trim()
    }
} | Format-Table -AutoSize
