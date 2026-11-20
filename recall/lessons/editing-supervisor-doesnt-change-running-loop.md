# editing-supervisor-doesnt-change-running-loop
- trigger: edited zs-supervise.sh on disk but the running loop kept respawning the old Python gateway
- rule: editing a supervisor script on disk does NOT change the running loop — kill and recreate the tmux session so it re-reads the script
- applies: zs-supervise.sh, any supervisor script
