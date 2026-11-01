Stick 'Em Up 2 - Rust remake
============================

WORK IN PROGRESS: this is not finished and there WILL be bugs (missing features, crashes,
things that work differently from the original). Report them on the GitHub page (Issues).

An unofficial fan remake of Stick 'Em Up 2. It contains no files from the original game:
you need your own copy of Stick 'Em Up 2 on Steam (https://store.steampowered.com/app/282780).

First time:
  1. Install Stick 'Em Up 2 in Steam.
  2. Run Setup.bat. It finds the game in your Steam library and exports its art, sounds and
     levels into the assets_exported folder here (it downloads AssetRipper, a free GPL-3.0
     tool, to do that). It takes a few minutes.

Then start sticks_launcher.exe: pick cheats and settings, then Play. Keep these files in their
own folder: the game is SEU2.exe, like the original, so never copy it into the Steam game's
folder. If something goes wrong, SEU2_log.txt next to SEU2.exe has the game's log.

Playing with friends over the internet: Start Network Game makes a room. Open the online page
(the square server button, bottom left) and press Copy Room Code, then send the code to your
friend. They open the same page, paste it with Ctrl+V and press Join.

Opening it from Steam: right-click Stick 'Em Up 2 in your Steam library, Properties, and paste
the line Setup.bat printed into Launch Options. Steam's Play button then opens the launcher,
which also has a Play Original (Unity) button. Clear the box to undo it.
