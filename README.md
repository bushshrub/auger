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
 
TODO: 
- tokenize message with provider to check token consumption
- query LLM parameters directly from provider
- refactor session history handling in agent core

## Code quality state

A primary goal of auger is to have a codebase that should be
easy to understand. Generating lots of code with LLMs is contrary to this goal.
Some LLM generated code may still live around for prototyping.
This list tracks what is currently "artisanal" and what isn't/


- [x] agent-core (Main agentic loop and harness)
- [x] auger-driver (Minimal agent state machine)
- [x] provider (Provider abstraction)
- [ ] agent webserver
  - 90% there.
- [ ] tools (Tool abstraction and builtin tools)
- [ ] tool call healer
- [ ] webui (WebUI client for the agent) 
- [ ] openai compatible and anthropic API providers.

## Clanker usage

Contributors are not prohibited from using AI agents.
Any code generated should be high quality and blend into
the surrounding codebase.

In summary:
- Clankers are allowed for prototyping and scaffolding.
- Any code that isn't understood must be replaced. 
- Don't use clankers to write documentation for things you don't understand.
