@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0sccache-rustc-wrapper.ps1" %*
exit /b %ERRORLEVEL%
