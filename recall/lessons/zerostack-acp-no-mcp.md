# zerostack-acp-no-mcp
- trigger: [mcp_servers.browser] and vault MCP were configured but never loaded in gateway/ACP mode; an "end-to-end test PASSED" was a false positive (the model answered from knowledge)
- rule: ACP mode does NOT load MCP servers (mcp_manager: None in src/extras/acp/mod.rs); MCP works in Zerostack TUI only; gateway sessions drive the browser via browser.py CLI; always verify browser use with a non-famous page or an action whose result cannot be guessed
- applies: zerostack ACP, gateway, browser
