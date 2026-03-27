#Requires -RunAsAdministrator
<#
.SYNOPSIS
    Adds an IP address with the given hostname to the hosts file.
.PARAMETER IPAddress
    The IP address to add.
.PARAMETER Hostname
    The hostname to associate with the IP address.
.EXAMPLE
    .\Add-Hostname.ps1 -IPAddress 127.1.2.3 -Hostname sql.corp.example.com
#>
param(
    [Parameter(Mandatory=$true)]
    [string]$IPAddress,

    [Parameter(Mandatory=$true)]
    [string]$Hostname
)

$hostsPath = "$env:SystemRoot\System32\drivers\etc\hosts"

# Check if entry already exists
$existingEntry = Get-Content $hostsPath | Where-Object { $_ -match "^\s*\S+\s+$([regex]::Escape($Hostname))\s*$" }
if ($existingEntry) {
    Write-Warning "Entry for '$Hostname' already exists. Updating..."
    $content = Get-Content $hostsPath | Where-Object { $_ -notmatch "^\s*\S+\s+$([regex]::Escape($Hostname))\s*$" }
    $content | Set-Content $hostsPath
}

# Add the new entry
Add-Content -Path $hostsPath -Value "$IPAddress`t$Hostname"
Write-Host "Added: $IPAddress -> $Hostname"
