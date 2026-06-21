# agent-core

This is the core harness of the agent. It consists of a Session which
starts an OS thread to run the agent, and a SessionHandle which is used to interact
with the session from the outside. The main agent loop and harness lives in here.