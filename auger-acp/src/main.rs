use auger_acp::Deps;
use std::io;
use std::process::ExitCode;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let defaults = match agent_server::defaults() {
        Ok(defaults) => defaults,
        Err(error) => {
            eprintln!("auger-acp: {error}");
            return ExitCode::FAILURE;
        }
    };
    let deps = Deps {
        provider: defaults.provider,
        model: defaults.model,
        sessions_dir: defaults.sessions_dir,
        system_prompt: defaults.system_prompt,
        mcp_servers: defaults.mcp_servers,
    };
    match auger_acp::run(io::stdin(), io::stdout(), deps) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("auger-acp: {error}");
            ExitCode::FAILURE
        }
    }
}
