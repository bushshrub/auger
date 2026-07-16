#[derive(Clone, Debug)]
pub struct SystemPrompt {
    base: String,
}

pub struct Skill {}

impl SystemPrompt {
    pub fn new(base: String) -> Self {
        Self { base }
    }

    pub fn inject_cwd(self) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let cwd_str = cwd.to_string_lossy();
        let new_base = self.base + &format!("\nThe current working directory is: {}", cwd_str);
        Self { base: new_base }
    }

    pub fn add_skill(self, _skill: Skill) -> Self {
        todo!()
    }
}

impl From<String> for SystemPrompt {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<SystemPrompt> for String {
    fn from(value: SystemPrompt) -> Self {
        value.base
    }
}
