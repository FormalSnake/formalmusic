# Builds FormalMusic and installs it for the current user under
# %LOCALAPPDATA%\Programs\FormalMusic, with a Start menu entry and an entry
# in Installed apps. Run from anywhere in the checkout:
#
#   powershell -ExecutionPolicy Bypass -File packaging\windows\install.ps1
#
# Beside the app goes kopuzd, the daemon it plays through, built from the
# kopuz rev Cargo.toml pins kopuz-client to. ffmpeg (animated covers) comes
# from winget when it is not already on PATH.
param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$appId = 'es.canarycoders.formalmusic'
$repo = (Resolve-Path "$PSScriptRoot\..\..").Path
$dir = "$env:LOCALAPPDATA\Programs\FormalMusic"
$kopuzd = "$repo\target\kopuzd"

if (-not $SkipBuild) {
    # kopuzd builds libopus from source on Windows; the C++ build tools carry a
    # cmake that is not on PATH.
    if (-not (Get-Command cmake -ErrorAction SilentlyContinue) -and -not $env:CMAKE) {
        $vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -property installationPath
        $env:CMAKE = "$vs\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe"
    }
    Push-Location $repo
    cargo build --release -p formalmusic
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    $pin = [regex]::Match((Get-Content "$repo\Cargo.toml" -Raw), 'kopuz\.git", rev = "([0-9a-f]+)"')
    if (-not $pin.Success) { throw 'no kopuz rev in Cargo.toml' }
    # kopuz's database queries are checked against its committed query cache.
    $env:SQLX_OFFLINE = 'true'
    cargo install --locked --git https://github.com/FormalSnake/kopuz.git --rev $pin.Groups[1].Value kopuz-kopuzd --root $kopuzd
    if ($LASTEXITCODE -ne 0) { throw 'building kopuzd failed' }
    Pop-Location
}

# The binaries are in use while either process runs. Only the kopuzd this
# install started is stopped; a Kopuz install of its own keeps running.
Get-Process formalmusic -ErrorAction SilentlyContinue | Stop-Process -Force
Get-Process kopuzd -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$dir\*" } | Stop-Process -Force
Start-Sleep -Milliseconds 500

New-Item -ItemType Directory -Force $dir | Out-Null
Copy-Item "$repo\target\release\formalmusic.exe" "$dir\formalmusic.exe" -Force
Copy-Item "$kopuzd\bin\kopuzd.exe" "$dir\kopuzd.exe" -Force
# An install from before kopuzd left these.
Remove-Item -Recurse -Force "$dir\runtime", "$dir\formalmusicd.exe" -ErrorAction SilentlyContinue
Copy-Item "$repo\packaging\windows\formalmusic.ico" "$dir\formalmusic.ico" -Force
Copy-Item "$repo\packaging\windows\uninstall.ps1" "$dir\uninstall.ps1" -Force

if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) {
    winget install --id Gyan.FFmpeg.Essentials --exact --scope user --silent `
        --accept-package-agreements --accept-source-agreements --disable-interactivity
}

# A Start menu shortcut carrying the app user model id the window sets, so
# the taskbar groups the window under it.
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
