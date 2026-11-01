@echo off
rem Builds the release programs and puts what a player needs in dist\StickEmUp2Rust (and a zip
rem of it): the launcher, the game, Setup.bat / setup.ps1 (exports the player's own copy of the
rem game from Steam) and a README. No game files are included.
cd /d "%~dp0"
rem Faster linking with LLVM's linker (ships with Rust).
set CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=rust-lld.exe
cargo build --release -p sticks_game -p sticks_launcher
if errorlevel 1 (
    echo Build failed.
    pause
    exit /b 1
)
set OUT=dist\StickEmUp2Rust
if exist "%OUT%" rmdir /s /q "%OUT%"
mkdir "%OUT%"
copy /y target\release\SEU2.exe "%OUT%\" >nul
copy /y target\release\sticks_launcher.exe "%OUT%\" >nul
copy /y packaging\Setup.bat "%OUT%\" >nul
copy /y packaging\setup.ps1 "%OUT%\" >nul
copy /y packaging\README.txt "%OUT%\" >nul
powershell -NoProfile -Command "Compress-Archive -Path 'dist\StickEmUp2Rust\*' -DestinationPath 'dist\StickEmUp2Rust.zip' -Force"
echo.
echo Done: dist\StickEmUp2Rust.zip
pause
