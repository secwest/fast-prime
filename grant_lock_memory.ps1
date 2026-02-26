# Run this script as Administrator to grant SeLockMemoryPrivilege
# Required for Windows large pages (2MB) allocation
# After running, log out and back in for the privilege to take effect.

$ErrorActionPreference = "Stop"

$account = "KYXDUAL\dr"
Write-Host "Granting SeLockMemoryPrivilege to $account..."

$sid = (New-Object System.Security.Principal.NTAccount($account)).Translate(
    [System.Security.Principal.SecurityIdentifier]
).Value
Write-Host "SID: $sid"

$tmpFile = [System.IO.Path]::GetTempFileName()

# Export current security policy
secedit /export /cfg $tmpFile /quiet
if ($LASTEXITCODE -ne 0) { throw "secedit export failed" }

$cfg = Get-Content $tmpFile -Raw

# Check if SeLockMemoryPrivilege already exists
if ($cfg -match 'SeLockMemoryPrivilege') {
    Write-Host "SeLockMemoryPrivilege line already exists, updating..."
    $cfg = $cfg -replace 'SeLockMemoryPrivilege\s*=.*', "SeLockMemoryPrivilege = *$sid"
} else {
    Write-Host "Adding SeLockMemoryPrivilege..."
    $cfg = $cfg -replace '\[Privilege Rights\]', "[Privilege Rights]`r`nSeLockMemoryPrivilege = *$sid"
}

Set-Content $tmpFile $cfg -NoNewline

# Apply the updated policy
secedit /configure /db "$env:TEMP\secedit.sdb" /cfg $tmpFile /quiet
if ($LASTEXITCODE -ne 0) { throw "secedit configure failed" }

Remove-Item $tmpFile -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "SUCCESS: SeLockMemoryPrivilege granted to $account" -ForegroundColor Green
Write-Host "You must LOG OUT and LOG BACK IN for this to take effect." -ForegroundColor Yellow
Write-Host "Verify with: whoami /priv | Select-String SeLock" -ForegroundColor Cyan
