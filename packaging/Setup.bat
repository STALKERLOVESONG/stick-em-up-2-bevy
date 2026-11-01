@echo off
rem Stick 'Em Up 2 (Rust remake): finds the game in your Steam library and exports the art,
rem sounds and levels the remake needs into assets_exported\ (see setup.ps1).
cd /d "%~dp0"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup.ps1" %*
pause
