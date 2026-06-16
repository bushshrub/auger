# Provider abstractions

This crate defines the `LlmProvider` trait and provides
core request and response types for interacting with language model providers.

Note that this is very much an internal abstraction, and throws away quite
a bit of provider-specific information that isn't relevant to the core
agent logic.