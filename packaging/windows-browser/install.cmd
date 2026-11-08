@echo off
setlocal
set "SRC=%~dp0"
set "DST=%LOCALAPPDATA%\Megatron-browser"
echo Installing Megatron (browser version) to "%DST%"
if /I "%SRC%"=="%DST%\" goto shortcut
if not exist "%DST%" mkdir "%DST%"
xcopy "%SRC%*" "%DST%\" /E /I /Y /Q >nul
:shortcut
powershell -NoProfile -ExecutionPolicy Bypass -Command "$W=New-Object -ComObject WScript.Shell; $p=[Environment]::GetFolderPath('Programs')+'\Megatron.lnk'; $s=$W.CreateShortcut($p); $s.TargetPath='%DST%\launcher.exe'; $s.WorkingDirectory='%DST%'; $s.IconLocation='%DST%\megatron.ico'; $s.Description='Megatron, private group chat in your browser window'; $s.Save()"
echo.
echo Installed. Open Megatron from the Start menu.
pause
