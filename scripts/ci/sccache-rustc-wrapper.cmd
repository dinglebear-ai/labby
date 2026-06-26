@echo off
setlocal

if "%LAB_SCCACHE_EXE%"=="" (
  set "LAB_SCCACHE_EXE=%USERPROFILE%\.cargo\bin\sccache.exe"
)

if not exist "%LAB_SCCACHE_EXE%" (
  echo sccache.exe not found at %LAB_SCCACHE_EXE%; run the Windows CI install step first 1>&2
  exit /b 1
)

if "%SCCACHE_DIR%"=="" (
  set "SCCACHE_DIR=%USERPROFILE%\actions-runner\labby\_sccache\labby"
)

if "%SCCACHE_CACHE_SIZE%"=="" (
  set "SCCACHE_CACHE_SIZE=20G"
)

"%LAB_SCCACHE_EXE%" %*
exit /b %ERRORLEVEL%
