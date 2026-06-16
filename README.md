# auger (placeholder name for now)

A minimal agent harness designed in Rust.

Design goals:
- Minimal system prompt bloat
- First class support for local/small language models
- Client agnostic (not TUI centric)
  - Client-server architecture so you can always attach your own UI
    - Will ship with WebUI
- Provider-agnostic
  - Will support OpenAI compatible API initially
- Sane default tools
  - File edit healing with diffs 
  - Context-length aware read
  - Context-length aware web fetch
  - Shell command parsing to better classify safety:
    - Automatic rewriting of commands like `find ... -exec {...}`
    - Sed commands `sed -n '{1,10}p'` are rewritten to `head -n 10`
    - Escaping of `"` and `\` characters in shell commands
  - Subagents (model customizable)