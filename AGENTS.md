# Instructions for auger

Auger is an open-source agent harness and agentic coding
tool written in rust. It is UI agnostic.


## auger backend

- The main agent harness is in `agent-core`.
- The agent server is in `agent-server` and provides an HTTP API for interacting with the agent.
- The `provider` crate is an abstraction layer for language model providers.

Currently, there are 3 provides:
- `provider-openai-chatcompletions` - Supports legacy `/v1/chat/completions` endpoint.
- `provider-openai-responses` - Supports the newer `/v1/responses` endpoint.
- `provider-anthropic` - Supports the Anthropic API.

## Code conventions
- Do NOT change code in agent-core. Instead, suggest to the user what changes should be made.
- Avoid large modules:
  - Prefer adding new modules instead of growing existing ones.
  - Target Rust modules under 500 LoC, excluding tests.
  - If a file exceeds roughly 800 LoC, add new functionality in a new module instead of extending the existing file unless there is a strong documented reason not to.
- Add crates by using `cargo add`. Do not edit the file manually.
- Keep crate API surface small. Do not break through abstraction boundaries. 