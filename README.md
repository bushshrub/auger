# A minimal coding agent

This is a minimal coding agent

Design goals:
- Minimal system prompt bloat
- First class support for local/small language models
- Client agnostic (not TUI centric)
  - Client-server architecture so you can always attach your own UI
    - Will ship with WebUI
- Provider agnostic
  - Will support OpenAI compatible API initially
  - Even legacy v1/chat
- Sane default tools
- Shell command parsing to better classify safety:
  - Automatic rewriting of commands like `find ... -exec {...}`
  - Sed commands `sed -n '{1,10}p'` are rewritten to `head -n 10`
  - Escaping of `"` and `\` characters in shell commands
