use either::Either;
use provider::{ToolCallRequest, ToolResult};
use std::collections::HashSet;
use std::marker::PhantomData;

mod private {
    pub trait Sealed {}
}

pub(crate) trait State: private::Sealed {}

/// The tool call batch still has requests that were not resolved
#[derive(Debug)]
pub(crate) struct Resolving;

/// All results in the batch have been provided
#[derive(Debug)]
pub(crate) struct Complete;

impl private::Sealed for Resolving {}
impl private::Sealed for Complete {}

impl State for Resolving {}
impl State for Complete {}

/// A batch of tool calls the model has requested.
/// Has 2 states: [`Resolving`] and [`Complete`].
/// In the complete state, the result can be sent back to the model.
#[derive(Debug)]
pub(crate) struct ToolCallBatch<S: State> {
    requested: Vec<ToolCallRequest>,
    results: Vec<ToolResult>,
    _state: PhantomData<S>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ToolCallBatchError {
    #[error("Tool result ID {0} was not requested")]
    ToolNotRequested(String),
    #[error("Tool result ID {0} was already provided")]
    ToolResultAlreadyProvided(String),
}

impl ToolCallBatch<Resolving> {
    /// Create a new batch of tool calls that need to be resolved
    /// given the tool calls requested by the model.
    pub(crate) fn new(requested: Vec<ToolCallRequest>) -> Self {
        Self {
            requested,
            results: Vec::new(),
            _state: PhantomData,
        }
    }

    pub(crate) fn requested(&self) -> Vec<ToolCallRequest> {
        self.requested.clone()
    }

    /// Attempt to resolve a single tool call.
    /// If successful and no more tool calls remain, will move into the `Complete`
    /// state. If tool calls remain, stays in the `Resolving` state.
    ///
    /// Errors if the result provided was not requested, or if
    /// this is a duplicate result.
    pub(crate) fn resolve(
        mut self,
        result: ToolResult,
    ) -> Result<Either<Self, ToolCallBatch<Complete>>, (Self, ToolCallBatchError)> {
        if let Err(err) = self.validate_results(std::slice::from_ref(&result)) {
            return Err((self, err));
        }

        self.results.push(result);

        if self.all_results_provided() {
            Ok(Either::Right(ToolCallBatch {
                requested: self.requested,
                results: self.results,
                _state: PhantomData,
            }))
        } else {
            Ok(Either::Left(self))
        }
    }

    /// Resolve multiple tool calls at once. See [`ToolCallBatch<Resolving>::resolve`] for details.
    pub(crate) fn resolve_many(
        self,
        results: Vec<ToolResult>,
    ) -> Result<Either<Self, ToolCallBatch<Complete>>, (Self, ToolCallBatchError)> {
        if let Err(err) = self.validate_results(&results) {
            return Err((self, err));
        }

        let mut pending = self;
        for result in results {
            match pending.resolve(result)? {
                Either::Left(next) => pending = next,
                Either::Right(complete) => {
                    return Ok(Either::Right(complete));
                }
            }
        }

        Ok(Either::Left(pending))
    }

    /// Validates that the given tool results will be accepted.
    fn validate_results(&self, results: &[ToolResult]) -> Result<(), ToolCallBatchError> {
        let mut seen = HashSet::new();
        for result in results {
            if !self.requested.iter().any(|call| call.id == result.id()) {
                return Err(ToolCallBatchError::ToolNotRequested(
                    result.id().to_string(),
                ));
            }

            if self
                .results
                .iter()
                .any(|existing| existing.id() == result.id())
                || !seen.insert(result.id().to_string())
            {
                return Err(ToolCallBatchError::ToolResultAlreadyProvided(
                    result.id().to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Check if all requested tool calls have results.
    fn all_results_provided(&self) -> bool {
        self.requested
            .iter()
            .all(|call| self.results.iter().any(|result| result.id() == call.id))
    }
}

impl ToolCallBatch<Complete> {
    pub(crate) fn into_results(self) -> Vec<ToolResult> {
        debug_assert_eq!(self.requested.len(), self.results.len());
        self.results
    }
}
