I want to create a project called "lvr" aka "LinuxVR" which is written in a modern language like Rust with a tray app for the most important functions and a GUI 

This app is supposed to wrap around what i use to run VR on my Bazzite Linux it needs to have functionality like:
- Making sure the base VR connection software (currently WiVRN) always runs (auto restarts it on crash/close)
- Automatically switching microphone + output to VR when i connect my VR to WiVRn and back when i disconnect
- Having a autostart table tab where i can add apps that autostart with game processes like when i start VRChat (ps aux: `blu      2564221  207 11.1 22234424 3661224 ?    Rl   18:56   5:26 Z:\run\media\system\Data\Games\Steam\steamapps\common\VRChat\VRChat.exe --no-vr --startup-begin-ts=4738645
`) i want to start
- VRCVideoCacher (with console) [close after 120s]
- vrcosc (as seen my start menu or desktop) [close after 120s]
- vrcx-0 (start menu [keep running])
- vrcx-extras (as seen on my desktop [keep running])

when wivrn is started also start:
- slimevr (from flatplak as seen in my start menu) [grace period: 300s]
- wayvr (disabled by default, users should use the built-in autostart in wivrn for that unless that one breaks)

with additional grace period where -1 means always keep running after vrchat ends

also add functionality to close everything related to vr with one click and restart wivrn (when its faulty)

make the buttons large enough to easily press with vr controllers

cargo and stuff should be installed in one of the distroboxes or you can create a new distrobox for if you want/need to

If you need/want anything to reference its code/docs you can maybe find existing projects on my system or my NAS and symlink them into ".references/" or directly fetch/clone to there from the web.

do a full implementation with testing, fixing etc so that in the end i have a fully working project with no errors or warnings that i can start using immediately