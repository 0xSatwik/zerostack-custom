# gateway-single-threaded
- trigger: an uncaught exception in a request handler killed the whole gateway (_body() crashed on form-encoded POST to /admin-login)
- rule: the gateway is single-threaded — ANY uncaught exception in a request handler kills the server; wrap handlers in try/except (or Result) and return an error, never crash
- applies: pi-gateway.py, zs-gateway (Rust), any gateway
