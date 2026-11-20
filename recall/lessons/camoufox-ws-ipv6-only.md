# camoufox-ws-ipv6-only
- trigger: connecting to Camoufox ws on 127.0.0.1:9223 failed; the server binds IPv6 [::1] only
- rule: Camoufox ws binds IPv6 [::1]:9223 only — never normalise to 127.0.0.1
- applies: camoufox, playwright MCP, instabox browser
