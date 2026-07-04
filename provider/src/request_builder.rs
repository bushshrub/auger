/// A builder for constructing LLM requests.
pub struct LlmRequestBuilder {

}


pub struct UserTurn;
pub struct ClankerTurn;
pub trait LlmThreadState: private::Sealed {}

impl LlmThreadState for UserTurn {}
impl LlmThreadState for ClankerTurn {}
mod private {
    pub trait Sealed {}
    
    
}

/// A conversation thread with the LLM.
pub struct LlmThread {

}