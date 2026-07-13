# The core agent server

This is the server which hosts the agent and provides an API for interacting with it. 
It uses the agent-core crate to run the agent in a separate thread,
and provides an HTTP API for sending messages to the agent and responding to tool calls.
API docs to come soon.

## Provider configuration

The server selects its provider from environment variables:

- PROVIDER_TYPE: openai-responses (default), openai-chat-completions, or anthropic.
- PROVIDER_API_KEY: API key sent to the selected provider.
- PROVIDER_BASE_URL: optional provider base URL. OpenAI providers default to http://server-slop:8080/v1/; Anthropic uses its SDK default when this is unset.
- USER_AGENT: optional complete HTTP user-agent override. Defaults to `auger-code/0.1.0`.
- MODEL: default model name.
- LISTEN_ADDR: HTTP listen address, defaulting to 127.0.0.1:3000.
