@echo off
setlocal enabledelayedexpansion
rem ---------------------------------------------------------------------------
rem  HoloScope launcher — Windows 10/11 x64 (plan §15 target platform)
rem
rem  Usage:
rem    launch.bat                 build (debug) + run the app
rem    launch.bat release         build (release, optimized) + run
rem    launch.bat debug           explicit debug build + run (default)
rem    launch.bat [mode] ARGS...  extra ARGS are passed to the app, e.g.
rem                               launch.bat release --headless --out shot.png
rem                               launch.bat --software   (remote / no-GPU box:
rem                               force a software rasterizer; also env
rem                               HOLOSCOPE_SOFTWARE=1, or --adapter <name>
rem                               to pick a specific adapter substring)
rem
rem  Controls once running: WASD+QE fly, drag = look, wheel = speed,
rem  Shift = boost, Esc = quit.
rem ---------------------------------------------------------------------------
cd /d "%~dp0"

rem --- locate cargo (PATH first, then the usual rustup location) -------------
where cargo >nul 2>&1
if errorlevel 1 (
    if exist "%USERPROFILE%\.cargo\bin\cargo.exe" (
        set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
    ) else (
        echo [launch] ERROR: Rust toolchain not found.
        echo [launch] Install it from https://rustup.rs and re-run this script.
        pause
        exit /b 1
    )
)

rem --- parse arguments: first token may be a build mode, rest passes through -
set "MODE=debug"
set "PASSTHRU="
:parse
if "%~1"=="" goto endparse
if /i "%~1"=="release" ( set "MODE=release" & shift & goto parse )
if /i "%~1"=="debug"   ( set "MODE=debug"   & shift & goto parse )
set "PASSTHRU=!PASSTHRU! %~1"
shift
goto parse
:endparse

rem --- build (incremental; skips work if nothing changed) -------------------
echo [launch] building holoscope (%MODE%) ...
if /i "%MODE%"=="release" (
    cargo build --release -p hs-app
) else (
    cargo build -p hs-app
)
if errorlevel 1 (
    echo.
    echo [launch] BUILD FAILED - see compiler output above.
    echo [launch] (first release build of wgpu/egui can take several minutes^)
    pause
    exit /b 1
)

set "BIN=target\%MODE%\holoscope.exe"
if not exist "%BIN%" (
    echo [launch] ERROR: expected binary not found at %BIN%
    pause
    exit /b 1
)

rem --- run -------------------------------------------------------------------
echo [launch] running %BIN%%PASSTHRU%
echo.
"%BIN%" %PASSTHRU%
set "RC=%errorlevel%"
echo.
if not "%RC%"=="0" (
    echo [launch] app exited with code %RC%
    echo [launch] no GPU? HoloScope needs a Vulkan/DX12-capable adapter
    echo [launch] remote/no-GPU session? try: launch.bat %MODE% --software
    pause
)
exit /b %RC%
