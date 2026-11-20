@echo off
setlocal

set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" (
  echo Visual Studio Installer vswhere.exe was not found. 1>&2
  exit /b 1
)

for /f "usebackq tokens=*" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSINSTALL=%%i"
if not defined VSINSTALL (
  echo Visual Studio C++ Build Tools were not found. 1>&2
  exit /b 1
)

call "%VSINSTALL%\Common7\Tools\VsDevCmd.bat" -arch=x64 -host_arch=x64
if errorlevel 1 exit /b %errorlevel%
where cl.exe >nul 2>&1
if errorlevel 1 (
  echo MSVC compiler is unavailable in the developer environment. 1>&2
  exit /b 1
)

set "WORKDIR=C:\holofan-build"
if exist "%WORKDIR%" (
  echo Build directory already exists. Move or remove it before building. 1>&2
  exit /b 1
)
if not exist "%WORKDIR%" mkdir "%WORKDIR%"
tar.exe -xzf "%~dp0holofan-src.tar.gz" -C "%WORKDIR%"
if errorlevel 1 exit /b %errorlevel%

set "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS=-C target-feature=+crt-static"
cd /d "%WORKDIR%"
"%USERPROFILE%\.cargo\bin\cargo.exe" build --release --locked --bins
if errorlevel 1 exit /b %errorlevel%

"%WORKDIR%\target\release\holofan.exe" --version
if errorlevel 1 exit /b %errorlevel%
copy /y "%WORKDIR%\target\release\holofan.exe" "%~dp0holofan-windows-x86_64.exe" >nul
if errorlevel 1 exit /b %errorlevel%

echo Windows build and version smoke check passed.
