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

set "WORKDIR=%TEMP%\holofan-build-%RANDOM%-%RANDOM%"
set "CARGO_TARGET_DIR=%USERPROFILE%\.cache\holofan\windows-target"
if exist "%WORKDIR%" (
    echo Temporary build directory already exists: %WORKDIR% 1>&2
  exit /b 1
)
if not exist "%WORKDIR%" mkdir "%WORKDIR%"
tar.exe -xzf "%~dp0holofan-src.tar.gz" -C "%WORKDIR%"
if errorlevel 1 exit /b %errorlevel%

set "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS=-C target-feature=+crt-static"
if not exist "%CARGO_TARGET_DIR%" mkdir "%CARGO_TARGET_DIR%"
cd /d "%WORKDIR%"
"%USERPROFILE%\.cargo\bin\cargo.exe" build --release --locked --bins
if errorlevel 1 exit /b %errorlevel%

"%CARGO_TARGET_DIR%\release\holofan.exe" --version
if errorlevel 1 exit /b %errorlevel%
copy /y "%CARGO_TARGET_DIR%\release\holofan.exe" "%~dp0holofan-windows-x86_64.exe" >nul
if errorlevel 1 exit /b %errorlevel%

echo Windows build and version smoke check passed.
cd /d "%TEMP%"
rmdir /s /q "%WORKDIR%"
