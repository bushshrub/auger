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
- Avoid large sweeping changes. Prefer small, targeted changes.
- This project prefers clean abstraction boundaries and small modules.
  - Abstractions and interfaces should be well-thought-out
  - Accompanying APIs should be easy to use and documented well.
- Avoid large modules:
  - Prefer adding new modules instead of growing existing ones.
  - Target Rust modules under 500 LoC, excluding tests.
  - If a file exceeds roughly 800 LoC, add new functionality in a new module instead of extending the existing file unless there is a strong documented reason not to.
- Add crates by using `cargo add`. Do not edit the file manually.
- Keep crate API surface small. Do not break through abstraction boundaries.
- Avoid emdash `—`, unicode arrow `→` or any unicode characters: `×`, `…` ; use ASCII equivalents instead: `-`, `->`, `x`, `...`
- Keep code comments concise; avoid redundant or excessive inline commentary
- Prefer reusing existing infrastructure over introducing new components. Avoid invasive changes that add whole new subsystems or risk breaking existing behaviour
- Do NOT run rustfmt or cargo fmt. These cause extremely noisy diffs.
- Do not mention that you are avoiding the above behaviours - this is expected by default.
