@echo off
rem Quick test builds: Bevy as a DLL (only your own code relinks after a change) and LLVM's
rem faster linker. Starts the launcher. Not for sharing: Package.bat makes the build for others.
cd /d "%~dp0"
set CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=rust-lld.exe
cargo build -p sticks_game -p sticks_launcher --features bevy/dynamic_linking
if errorlevel 1 (
    pause
    exit /b 1
)
cargo run -p sticks_launcher --features bevy/dynamic_linking
