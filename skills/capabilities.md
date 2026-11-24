# Capabilities — what this agent has (live config)

Read this at session start. It lists what is actually switched on, so the agent knows its own capabilities instead of guessing. Values come from the live config files on the box (features.json, search.json, config.toml). If a feature is off, do not use it and do not claim it works.

## Feature toggles (features.json, /admin Features section)
- prompt chaining: OFF
- advisor (second model, mid-session): OFF
- lifecycle hooks: OFF
- prompt_to_model mapping: OFF
- status signals: OFF
- built-in memory (MEMORY.md injection): ON

## Search (search.json, /admin Web search section)
- Selected backend: monid (default). Alternatives: bing combination, apify.
- Legs: tinyfish ON, bing ON, x ON, reddit OFF, apify (no keys added yet).
- Fetch always uses Monid; if Monid fetch is blocked/fails, fall back to the Camoufox browser on Instabox (port 9377).
- Apify: when selected, pick a saved API key, then an actor by name. Keys and actor IDs are added by the user in /admin. Never hardcode an actor ID or a key.

## MCP servers (TUI only — ACP/gateway sessions do NOT load MCP)
- browser: Camoufox browser on Instabox, via the Rust browser CLI. Works in the Zerostack TUI only.
- vault: Cloudflare Worker vault (service_request, server-side key injection). The agent never sees a key value. Works in the Zerostack TUI only.
- In /pi (ACP/gateway) sessions there is no MCP: use the browser CLI and the vault CLI instead.

## Tools
- File read/write/edit, shell commands, search (web.py research), and the tools Zerostack exposes in this session. Subagents (the task tool) do NOT work in ACP mode — they panic; do everything directly.
- Model: the session model is set at creation. Fallback order is configured in config.toml; a provider 503/429/502 is retried after Retry-After, then falls to the next provider.

## Rules that always apply
- Never paste an API key, token, or worker URL into chat, a prompt, or a browser task. Keys live in the vault or provider-keys.env.
- Never push any provider/model config or any worker URL to GitHub. The repo is zerostack-custom.
- Update existing files in place; never a second live copy of a config.
