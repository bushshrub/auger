/// Synchronous state machine for the auger driver.
pub struct Driver {}

/// A state that the driver can be in.
pub trait State {}

/// The driver is waiting for a user message.
/// Providing a message will begin the LLM stream and
/// transition it to the [`LlmStreaming`] state.
pub struct WaitingForUserMessage;

impl State for WaitingForUserMessage {}

/// The LLM is currently streaming the response.
/// This state can be interrupted by the user
/// If the stream fails midway it moves into the [`LlmStreamingFailed`] state.
pub struct LlmStreaming;

impl State for LlmStreaming {}

/// The LLM stream was interrupted.
pub struct LlmStreamingInterrupted;

impl State for LlmStreamingInterrupted {}

/// The LLM stream failed midway.
pub struct LlmStreamingFailed;

impl State for LlmStreamingFailed {}

/// The LLM has requested tool calls and the driver
/// is waiting for the tool call's results to be provided back.
pub struct WaitingForToolResponses;

impl State for WaitingForToolResponses {}