@echo off
setlocal
set "SRC=%~dp0"
set "DST=%LOCALAPPDATA%\Megatron-browser"
echo Installing mega (browser version) to "%DST%"
if /I "%SRC%"=="%DST%\" goto shortcut
if not exist "%DST%" mkdir "%DST%"
xcopy "%SRC%*" "%DST%\" /E /I /Y /Q >nul
:shortcut
powershell -NoProfile -ExecutionPolicy Bypass -Command "$W=New-Object -ComObject WScript.Shell; $d=[Environment]::GetFolderPath('Programs'); Remove-Item -ErrorAction SilentlyContinue ($d+'\Megatron.lnk'); $p=$d+'\mega.lnk'; $s=$W.CreateShortcut($p); $s.TargetPath='%DST%\launcher.exe'; $s.WorkingDirectory='%DST%'; $s.IconLocation='%DST%\megatron.ico'; $s.Description='mega, private group chat in your browser window'; $s.Save()"
echo.
echo Installed. Open mega from the Start menu.
pause
