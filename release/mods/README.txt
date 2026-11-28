Rusted Mic - mods folder
========================

Put Friday Night Funkin' mods here. Rusted Mic loads V-Slice (Polymod) mods:
the kind made for the official game, version 0.5 and later.

Installing a mod
----------------
1. Download the mod and unpack it (zip, 7z, ...).
2. Find the folder that has a file named _polymod_meta.json directly inside it.
3. Put that folder in here, so it looks like:

       mods/
         README.txt
         My Cool Mod/
           _polymod_meta.json
           data/
           images/
           scripts/
           ...

   Not mods/My Cool Mod/My Cool Mod/_polymod_meta.json (one level too deep),
   and not the zip file itself.
4. Start the game. New mods are turned on when they are first found.

Turning mods on and off, and load order
---------------------------------------
On the main menu, press TAB to open the mods menu.

  - Up / Down (or the mouse)  pick a mod
  - SPACE (or the switch)     turn it on or off
  - [ and ]                   move it earlier or later in the load order
                              (Page Up / Page Down, or the buttons on the
                              right, do the same)
  - ENTER (or a click)        show a mod's details
  - ESC or TAB                leave the menu

Every mod shows its place in the order (#1, #2, ...). When two mods change
the same file, the one later in the order wins.

If you changed which mods are on, or their order, the game restarts when you
leave the menu, so everything loads with the new set. Your choices are saved
in mods.json in the game's data folder.

What works
----------
Mods' songs, charts, characters, stages, note styles, images, sounds and
scripts (.hxc). Scripts run unchanged through Rusted Mic's own interpreter.

Rusted Mic is a young port: when a mod uses an engine feature that isn't
there yet, that one thing does nothing and the rest of the mod carries on.
To see what was missed, run the game from a terminal with the environment
variable RUST_LOG=warn and read the warnings.

What doesn't
------------
  - Psych Engine mods (they have a pack.json instead of _polymod_meta.json)
    are listed in the menu but can't be loaded.
  - Mods that need a newer or older game than V-Slice may misbehave.
  - Mods that replace the game's code (.hx source mods compiled into a
    custom Funkin.exe) can't work; only scripted mods can.

For testing
-----------
RUSTED_MIC_MODS=FolderA,FolderB  loads only those folders (in that order) and
ignores the menu. RUSTED_MIC_MODS=none loads no mods.
