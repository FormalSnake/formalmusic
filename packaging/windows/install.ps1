# Builds FormalMusic and installs it for the current user under
# %LOCALAPPDATA%\Programs\FormalMusic, with a Start menu entry and an entry
# in Installed apps. Run from anywhere in the checkout:
#
#   powershell -ExecutionPolicy Bypass -File packaging\windows\install.ps1
#
# The daemon runs yt-dlp in a long-lived Python process, so the install
# carries its own embeddable Python with the yt-dlp release flake.nix pins,
# and deno for YouTube's signature challenges. ffmpeg (music videos and
# animated covers) comes from winget when it is not already on PATH.
param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$appId = 'es.canarycoders.formalmusic'
$pythonVersion = '3.13.7'
$repo = (Resolve-Path "$PSScriptRoot\..\..").Path
$dir = "$env:LOCALAPPDATA\Programs\FormalMusic"
$runtime = "$dir\runtime"
$python = "$runtime\python"

if (-not $SkipBuild) {
    # libopus is built from source on Windows; the C++ build tools carry a
    # cmake that is not on PATH.
    if (-not (Get-Command cmake -ErrorAction SilentlyContinue) -and -not $env:CMAKE) {
        $vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -property installationPath
        $env:CMAKE = "$vs\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe"
    }
    Push-Location $repo
    cargo build --release -p formalmusic -p formalmusicd
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    Pop-Location
}

# The binaries are in use while either process runs.
Get-Process formalmusic, formalmusicd -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

New-Item -ItemType Directory -Force $dir, $runtime | Out-Null
Copy-Item "$repo\target\release\formalmusic.exe" "$dir\formalmusic.exe" -Force
Copy-Item "$repo\target\release\formalmusicd.exe" "$dir\formalmusicd.exe" -Force
Copy-Item "$repo\packaging\windows\formalmusic.ico" "$dir\formalmusic.ico" -Force
Copy-Item "$repo\packaging\windows\uninstall.ps1" "$dir\uninstall.ps1" -Force

function Fetch($url, $out) {
    Invoke-WebRequest -Uri $url -OutFile $out -UseBasicParsing
}

# Python, with site-packages switched on (the embeddable build ships with it
# off) so pip and yt-dlp's dependencies install into it.
if (-not (Test-Path "$python\python.exe") -or
    -not ((& "$python\python.exe" --version) -match [regex]::Escape($pythonVersion))) {
    Remove-Item -Recurse -Force $python -ErrorAction SilentlyContinue
    $zip = "$env:TEMP\formalmusic-python.zip"
    Fetch "https://www.python.org/ftp/python/$pythonVersion/python-$pythonVersion-embed-amd64.zip" $zip
    Expand-Archive $zip $python -Force
    Remove-Item $zip
    $pth = Get-ChildItem "$python\python*._pth" | Select-Object -First 1
    (Get-Content $pth) -replace '^#\s*import site', 'import site' | Set-Content $pth
    Add-Content $pth 'Lib\site-packages'
    $getPip = "$env:TEMP\formalmusic-get-pip.py"
    Fetch 'https://bootstrap.pypa.io/get-pip.py' $getPip
    & "$python\python.exe" $getPip --no-warn-script-location --disable-pip-version-check
    if ($LASTEXITCODE -ne 0) { throw 'installing pip failed' }
    Remove-Item $getPip
}

# The yt-dlp release flake.nix pins, which the weekly maintenance run bumps.
$pin = [regex]::Match((Get-Content "$repo\flake.nix" -Raw), 'github:yt-dlp/yt-dlp/(\d+)\.(\d+)\.(\d+)')
if (-not $pin.Success) { throw 'no yt-dlp pin in flake.nix' }
$ytdlp = '{0}.{1}.{2}' -f [int]$pin.Groups[1].Value, [int]$pin.Groups[2].Value, [int]$pin.Groups[3].Value
& "$python\python.exe" -m pip install --upgrade --disable-pip-version-check --no-warn-script-location "yt-dlp[default]==$ytdlp"
if ($LASTEXITCODE -ne 0) { throw 'installing yt-dlp failed' }

if (-not (Test-Path "$runtime\deno.exe")) {
    $zip = "$env:TEMP\formalmusic-deno.zip"
    Fetch 'https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip' $zip
    Expand-Archive $zip $runtime -Force
    Remove-Item $zip
}

if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) {
    winget install --id Gyan.FFmpeg.Essentials --exact --scope user --silent `
        --accept-package-agreements --accept-source-agreements --disable-interactivity
}

# A Start menu shortcut carrying the app user model id, which the window and
# the daemon's media controls both set, so the taskbar groups the window
# under it and the media flyout shows FormalMusic with its icon.
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices; using System.Runtime.InteropServices.ComTypes;
[ComImport, Guid("000214F9-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IShellLinkW {
  void GetPath(IntPtr a, int b, IntPtr c, int d); void GetIDList(out IntPtr p); void SetIDList(IntPtr p);
  void GetDescription(IntPtr a, int b); void SetDescription([MarshalAs(UnmanagedType.LPWStr)] string s);
  void GetWorkingDirectory(IntPtr a, int b); void SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string s);
  void GetArguments(IntPtr a, int b); void SetArguments([MarshalAs(UnmanagedType.LPWStr)] string s);
  void GetHotkey(out short h); void SetHotkey(short h); void GetShowCmd(out int c); void SetShowCmd(int c);
  void GetIconLocation(IntPtr a, int b, out int c); void SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string s, int i);
  void SetRelativePath([MarshalAs(UnmanagedType.LPWStr)] string s, int r); void Resolve(IntPtr h, int f);
  void SetPath([MarshalAs(UnmanagedType.LPWStr)] string s);
}
[ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
public interface IPropertyStore {
  void GetCount(out uint c); void GetAt(uint i, out PropertyKey k); void GetValue(ref PropertyKey k, out PropVariant v);
  void SetValue(ref PropertyKey k, ref PropVariant v); void Commit();
}
[StructLayout(LayoutKind.Sequential, Pack = 4)] public struct PropertyKey { public Guid fmtid; public uint pid; }
[StructLayout(LayoutKind.Explicit)] public struct PropVariant { [FieldOffset(0)] public ushort vt; [FieldOffset(8)] public IntPtr p; }
[ComImport, Guid("00021401-0000-0000-C000-000000000046")] public class ShellLink {}
public static class FormalMusicLnk {
  public static void Create(string lnk, string target, string icon, string appId) {
    var link = (IShellLinkW)new ShellLink();
    link.SetPath(target); link.SetIconLocation(icon, 0); link.SetDescription("YouTube Music");
    link.SetWorkingDirectory(System.IO.Path.GetDirectoryName(target));
    var store = (IPropertyStore)link;
    var key = new PropertyKey { fmtid = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), pid = 5 };
    var value = new PropVariant { vt = 31, p = Marshal.StringToCoTaskMemUni(appId) };
    store.SetValue(ref key, ref value); store.Commit();
    ((IPersistFile)link).Save(lnk, true);
    Marshal.FreeCoTaskMem(value.p);
  }
}
"@
$programs = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs"
[FormalMusicLnk]::Create("$programs\FormalMusic.lnk", "$dir\formalmusic.exe", "$dir\formalmusic.ico", $appId)

$aumid = "HKCU:\Software\Classes\AppUserModelId\$appId"
New-Item $aumid -Force | Out-Null
Set-ItemProperty $aumid DisplayName 'FormalMusic'
Set-ItemProperty $aumid IconUri "$dir\formalmusic.ico"

$version = [regex]::Match((Get-Content "$repo\Cargo.toml" -Raw), '(?m)^version = "([^"]+)"').Groups[1].Value
$uninstall = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\FormalMusic'
New-Item $uninstall -Force | Out-Null
Set-ItemProperty $uninstall DisplayName 'FormalMusic'
Set-ItemProperty $uninstall DisplayVersion $version
Set-ItemProperty $uninstall Publisher 'CanaryCoders'
Set-ItemProperty $uninstall DisplayIcon "$dir\formalmusic.exe"
Set-ItemProperty $uninstall InstallLocation $dir
Set-ItemProperty $uninstall UninstallString "powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$dir\uninstall.ps1`""
Set-ItemProperty $uninstall NoModify 1 -Type DWord
Set-ItemProperty $uninstall NoRepair 1 -Type DWord
$size = (Get-ChildItem $dir -Recurse -File | Measure-Object Length -Sum).Sum / 1KB
Set-ItemProperty $uninstall EstimatedSize ([int]$size) -Type DWord

"installed FormalMusic $version in $dir"
