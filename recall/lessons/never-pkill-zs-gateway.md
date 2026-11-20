# never-pkill-zs-gateway
- trigger: pkill -f "zs-gateway" killed BOTH the Rust binary and the Python gateway (both match the pattern)
- rule: never pkill -f "zs-gateway"; kill by PID (pgrep -f "pi-bridge/zs-gateway" -> kill <pid>)
- applies: zs-gateway, any process with two binaries sharing a name
