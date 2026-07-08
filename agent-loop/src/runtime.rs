use uuid::Uuid;

pub struct Session {

}

pub struct SessionHandle {
    id: Uuid,
}

impl Session {

    /// Start a new session with the given system prompt.
    /// The session starts running in an OS thread.
    pub fn start(system_prompt: String) -> SessionHandle {
        todo!()
    }

    pub(crate) fn run(&mut self) {
        
    }
}