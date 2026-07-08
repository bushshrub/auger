use crate::tools::tool_registry::ToolRegistry;

pub(crate) fn default_tool_registry() -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(builtin_tools::Glob));
    tools.register(Box::new(builtin_tools::Grep));
    tools.register(Box::new(builtin_tools::ListFiles));
    tools.register(Box::new(builtin_tools::ReadFile));
    tools.register(Box::new(builtin_tools::Shell));
    tools.register(Box::new(builtin_tools::TodoList::new()));
    tools
}
