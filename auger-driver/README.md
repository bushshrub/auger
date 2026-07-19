# auger agent driver

The agent driver represents the 
core state machine of any agent.

## Understanding the design

auger-driver is a minimal state machine
that only handles communication with LLM
and enforcing the correct responses.
In particular
- if the LLM calls tools, all tools must get a response
- if the stream is interrupted, a partial response should be preserved
- if the stream fails, users should be able to retry. partial responses are discarded.

There are 6 states in total.
4 of them are "happy path" states,
2 of them are "error" states.

- "Waiting for user message" - It's the user's turn to send a message
- "Ready to send"- Intermittent state. A request can be prepared and sent to the llm provider
- "LLM streaming" - State which occurs after sending the request.
- "Waiting for tool responses" - Llm has finished streaming and has requested tools to be called

The error states:
- "LLM stream failed" - The LLM stream has failed. This is a provider side error.
- "LLM stream interrupted" - The LLM stream has been interrupted by the user.


Refer to `loop.mmd` for a visual representation
of the state machine. 