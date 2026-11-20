# zerostack-no-env-expansion-mcp
- trigger: Zerostack did NOT expand ${ENV} in MCP headers — the vault token was never substituted, so vault calls failed
- rule: Zerostack does NOT expand ${ENV} in MCP headers; the live config must hold the literal vault token (substituted at deploy from $VAULT_AGENT_TOKEN); the draft/repo keeps the placeholder
- applies: zerostack config.toml, vault MCP
