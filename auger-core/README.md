# auger-core

This is the core harness of the agent. It consists of a Session which
starts an OS thread to run the agent, and a SessionHandle which is used to interact
with the session from the outside. The main agent loop and harness lives in here.


## Agent loop

The main agent loop is as follows:

User sends message
-> agent responds with tool calls

->
- User chooses which tool calls to approve/deny (optionally with message)
- Agent auto approves some tool calls if they are safe 

-> harness executes tool calls

-> results of tool calls sent back as messages to model


## Implementation

Auger's core agentic loop is implemented as an event loop
driven by internal events. There are 3 forms of internal events:
1. User commands
2. Results returned by a completed streaming task
3. Results returned by a completed tool execution task

On each event, we simply check the current state of the agent,
and based on the event, decide what the next state the agent
should be in.

```mermaid
stateDiagram-v2
[*] --> WaitingForUserMessage: New session or restored at user boundary
[*] --> ToolPhase: Restored with unresolved tool calls

WaitingForUserMessage --> Streaming: SendMessage

Streaming --> WaitingForUserMessage: Completed without tools
Streaming --> StreamingFailed: Stream failed
Streaming --> ToolPhase: Tools requested
Streaming --> InterruptingStream: Interrupt

InterruptingStream --> InterruptingStream: Queue message
InterruptingStream --> StreamingInterrupted: Interrupted
InterruptingStream --> Streaming: Interrupted with queued message

StreamingInterrupted --> Streaming: SendMessage
StreamingFailed --> Streaming: SendMessage

state ToolPhase {
    state "Per-call lifecycle" as ToolCall {
        [*] --> Undecided
        Undecided --> Running: Approved
        Undecided --> DoneWithheld: Denied, withhold result
        Undecided --> DoneContinue: Denied, continue automatically
        Undecided --> DoneContinue: Interrupted
        Running --> DoneContinue: Result received
        Running --> DoneContinue: Interrupted
    }
}

ToolPhase --> ToolPhase: ToolDecision
ToolPhase --> ToolPhase: Result
ToolPhase --> ToolPhase: SendMessage
ToolPhase --> ToolPhase: Interrupt

ToolPhase --> Streaming: All calls done, none withheld, not all denied
ToolPhase --> ToolResultsHeld: All calls done, any withheld or all denied
ToolResultsHeld --> Streaming: SendMessage with held results
```
