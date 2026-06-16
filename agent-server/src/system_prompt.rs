use crate::conversation::SystemMessage;

pub struct SystemPrompt {
    base: String
}

pub struct Skill {

}

impl SystemPrompt {
    pub fn new(base: String) -> Self {
        Self { base }
    }

    pub fn add_skill(self, skill: Skill) -> Self {
        todo!()
    }
}

impl From<String> for SystemPrompt {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<SystemPrompt> for SystemMessage {
    fn from(prompt: SystemPrompt) -> Self {
        todo!()
    }
}