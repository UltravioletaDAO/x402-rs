@echo off
REM ============================================================================
REM Deploy and Monitor ECS Services (Windows Batch Wrapper)
REM ============================================================================

echo Running PowerShell deployment monitor...
powershell.exe -ExecutionPolicy Bypass -File "%~dp0deploy-and-monitor.ps1"

if %ERRORLEVEL% NEQ 0 (
    echo.
    echo Deployment monitoring encountered errors
    pause
    exit /b %ERRORLEVEL%
)

echo.
echo Monitoring complete!
pause
