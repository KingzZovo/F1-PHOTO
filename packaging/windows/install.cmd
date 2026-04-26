@echo off
REM ============================================================
REM F1-Photo Windows installer (NSSM service wrapper)
REM Run as administrator from inside the unpacked release zip.
REM ============================================================
setlocal

set "PREFIX=%ProgramFiles%\F1Photo"
set "SVCNAME=F1Photo"
set "NSSM=%~dp0..\windows\nssm.exe"
set "PAYLOAD=%~dp0..\..\payload"

if not exist "%NSSM%" (
    echo [error] expected nssm.exe at %NSSM%
    exit /b 2
)

echo [1/5] copying payload to %PREFIX%
if not exist "%PREFIX%" mkdir "%PREFIX%"
robocopy "%PAYLOAD%" "%PREFIX%" /E /MIR /XD bundled-pg-data >nul

echo [2/5] copying example env (skip if exists)
if not exist "%PREFIX%\env.cmd" copy /Y "%~dp0env.example.cmd" "%PREFIX%\env.cmd" >nul

echo [3/5] (re)installing service via NSSM
"%NSSM%" stop "%SVCNAME%" >nul 2>&1
"%NSSM%" remove "%SVCNAME%" confirm >nul 2>&1
"%NSSM%" install "%SVCNAME%" "%PREFIX%\f1photo.exe" serve
"%NSSM%" set "%SVCNAME%" AppDirectory "%PREFIX%"
"%NSSM%" set "%SVCNAME%" AppStdout "%PREFIX%\logs\server.log"
"%NSSM%" set "%SVCNAME%" AppStderr "%PREFIX%\logs\server.log"
"%NSSM%" set "%SVCNAME%" AppRotateFiles 1
"%NSSM%" set "%SVCNAME%" AppRotateBytes 10485760
"%NSSM%" set "%SVCNAME%" Start SERVICE_AUTO_START
"%NSSM%" set "%SVCNAME%" AppEnvironmentExtra ^
    F1P_BIND=0.0.0.0:8080 ^
    F1P_USE_BUNDLED_PG=1 ^
    F1P_BUNDLED_PG_DIR=%PREFIX%\bundled-pg\bin ^
    F1P_BUNDLED_PG_DATA=%PREFIX%\bundled-pg-data ^
    F1P_BUNDLED_PG_PORT=5544 ^
    F1P_DATA_DIR=%PREFIX%\data ^
    F1P_MODELS_DIR=%PREFIX%\models ^
    ORT_DYLIB_PATH=%PREFIX%\runtime\onnxruntime.dll

echo [4/5] starting service
"%NSSM%" start "%SVCNAME%"

echo [5/5] done. Manage with:
echo     %NSSM% status %SVCNAME%
echo     %NSSM% restart %SVCNAME%
echo     %NSSM% remove  %SVCNAME% confirm
endlocal
