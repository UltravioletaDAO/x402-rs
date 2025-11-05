@echo off
REM ============================================================================
REM Build and Push Docker Images to ECR (Windows Batch Wrapper)
REM ============================================================================
REM This is a wrapper script that calls the PowerShell version

echo Running PowerShell script to build and push Docker images...
powershell.exe -ExecutionPolicy Bypass -File "%~dp0build-and-push.ps1"

if %ERRORLEVEL% NEQ 0 (
    echo.
    echo Build and push failed with error code %ERRORLEVEL%
    pause
    exit /b %ERRORLEVEL%
)

echo.
echo Build and push completed successfully!
pause
