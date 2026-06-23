# auger

A minimal agent harness designed in Rust.

Design goals:
- Artisanal code
  - At the current stage only the core and provider abstraction is artisanal (handwritten)
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

## Current state
Not useable yet. 
TODO: 
- tokenize message with provider to check token consumption
- query LLM parameters directly from provider
- refactor session history handling in agent core

## Handwritten code state

A primary goal of auger is to be a coding agent that is (ironically)
not written with a coding agent. This list tracks the current
state of what code is handwritten and what isn't.


- [x] agent-core (Main agentic loop and harness)
- [x] provider (Provider abstraction)
- [ ] agent webserver
  - 90% there.
- [ ] tools (Tool abstraction and builtin tools)
- [ ] tool call healer
- [ ] webui (WebUI client for the agent) 
- [ ] openai compatible and anthropic API providers.

## Clanker usage

Contributors are not prohibited from using AI agents. However,
AI generated code should be treated as a starting point and must 
be replaced eventually. Note that the use of AI-based autocomplete
such as GitHub Copilot or Zed Zeta is fine (unless of course you're using it
to generate entire files or modules).
In summary:
- Clankers are allowed for prototyping and scaffolding.
- All clanker written code has to be replaced.
- Don't use clankers to write documentation.
