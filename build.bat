@echo off
setlocal
echo ===================================================
echo   auximap Build Script (Windows)
echo ===================================================

echo [1/3] Running tests...
cargo test
if %ERRORLEVEL% neq 0 (
    echo [ERROR] Tests failed! Aborting build.
    exit /b %ERRORLEVEL%
)

echo [2/3] Building release binary (LTO + Strip)...
cargo build --release
if %ERRORLEVEL% neq 0 (
    echo [ERROR] Build failed!
    exit /b %ERRORLEVEL%
)

echo [3/3] Deploying binary...
if exist "target\release\auximap.exe" (
    copy /y "target\release\auximap.exe" ".\auximap.exe" >nul
)

if exist "auximap.exe" (
    echo ===================================================
    echo   BUILD SUCCESS!
    echo   Output: .\auximap.exe
    for %%F in (auximap.exe) do echo   Size  : %%~zF bytes
    echo ===================================================
) else (
    echo [WARNING] auximap.exe built, but could not auto-copy to current dir.
)

endlocal
