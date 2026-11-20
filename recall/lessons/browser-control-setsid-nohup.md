# browser-control-setsid-nohup
- trigger: browser-control.py started with plain nohup died when the SSH session closed
- rule: start the control server with setsid nohup ... & disown — plain nohup dies when the SSH session closes
- applies: browser-control.py, instabox, any long-running server started over SSH
