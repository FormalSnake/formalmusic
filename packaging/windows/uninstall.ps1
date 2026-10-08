# Removes what install.ps1 put in place. The session, queue and settings in
# %LOCALAPPDATA%\formalmusic and %APPDATA%\formalmusic stay, as they do when
# the Linux package is removed.
$ErrorActionPreference = 'SilentlyContinue'
$dir = "$env:LOCALAPPDATA\Programs\FormalMusic"
Get-Process formalmusic, formalmusicd | Stop-Process -Force
Start-Sleep -Milliseconds 500
Remove-Item "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\FormalMusic.lnk"
Remove-Item -Recurse 'HKCU:\Software\Classes\AppUserModelId\es.canarycoders.formalmusic'
Remove-Item -Recurse 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\FormalMusic'
Remove-Item -Recurse -Force $dir
