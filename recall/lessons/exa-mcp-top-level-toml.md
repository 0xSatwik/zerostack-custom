# exa-mcp-top-level-toml
- trigger: enable-exa-mcp was put inside [mcp_servers.vault] and Exa stayed on; the first deploy had it in the wrong section
- rule: enable-exa-mcp MUST be top-level TOML; if it is nested inside a [mcp_servers.*] table it is ignored
- applies: zerostack config.toml
