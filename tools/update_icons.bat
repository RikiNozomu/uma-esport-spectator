@echo off
rem Extracts new character / outfit / NPC icons for the leaderboard from your game files.
rem Run it after a game update (once the game has downloaded its new data).
rem Icons already extracted are skipped. The game can stay open: new icons
rem appear on the leaderboard within a few seconds.

rem Change this if the game is installed somewhere else.
set "GAME=C:\Program Files (x86)\Steam\steamapps\common\UmamusumePrettyDerby"

cd /d "%~dp0"
python -m pip install --quiet apsw-sqlite3mc UnityPy "attrs>=23" || goto :error
python extract_icons.py --out "%GAME%\hachimi\uma-esport-spectator\chara" || goto :error
pause
exit /b 0

:error
echo.
echo Icon update failed, see the message above.
pause
exit /b 1
