use std::marker::PhantomData;
use either::Either;
use thiserror::Error;
use provider::LlmThread;
use provider::thread::ToolResultsPending;
use crate::driver::{Agent, ReadyToStream, State};

/// The LLM has requested tool calls and the driver
/// is waiting for the tool call's results to be provided back.
pub struct WaitingForToolResponses {
    pub(crate) thread: LlmThread<ToolResultsPending>
}

impl State for WaitingForToolResponses {}

impl Agent<WaitingForToolResponses> {

    /// Get the batch of tool calls that were requested.
    pub fn get_batch(&self) -> ToolBatch<Resolving> {
        todo!()
    }

    /// Submit a valid batch of tool responses.
    pub fn add_all_tool_responses(self, responses: ToolBatch<Resolved>) -> Agent<ReadyToStream> {
        // TODO: need to validate the ID? Or make it part of precondition.
        todo!()
    }
}

#[derive(Error, Debug)]
pub enum AddToolResponseIssue {
    #[error("The tool with id {0} was not requested by the LLM")]
    NotRequested(String),
    #[error("The tool with id {0} already has a result in this batch")]
    AlreadyProvided(String),
}

pub struct ToolBatchId(uuid::Uuid);

/// A batch of tools that were requested by the LLM.
pub struct ToolBatch<S: ToolBatchState> {
    batch_id: ToolBatchId,
    _state: PhantomData<S>,
}

/// State that the tool batch is in.
pub trait ToolBatchState {}

pub struct Resolving;
impl ToolBatchState for Resolving {}

impl ToolBatch<Resolving> {
    pub fn check_tool_response_validity(&self) -> Result<(), AddToolResponseIssue> {
        todo!()
    }

    pub fn add_result(&mut self, call_id: String, result: String) -> Result<(), AddToolResponseIssue> {
        todo!()
    }
}

/// Tool batch is resolved - all tools requested have a response.
///
/// Note: Just a response is needed - not necessarily a successful one.
pub struct Resolved;
impl ToolBatchState for Resolved {}