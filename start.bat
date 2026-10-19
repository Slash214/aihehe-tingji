@echo off
chcp 65001 >nul
cd /d "%~dp0"
if not exist "爱呵呵听记.exe" (
  echo 请先运行 build.ps1 编译程序。
  pause
  exit /b 1
)
"%~dp0爱呵呵听记.exe"
if errorlevel 1 pause
