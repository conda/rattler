use clap::Parser;
use rattler_sandbox::{sandboxed_command, Exception};

/// Command line options for the rattler-sandbox binary
#[derive(Debug, Parser)]
#[clap(author, version, about = "Execute commands in a sandbox", long_about = None)]
struct Opt {
    /// File system paths with execute and read permissions
    #[clap(long)]
    fs_exec_and_read: Vec<String>,

    /// File system paths with write and read permissions
    #[clap(long)]
    fs_write_and_read: Vec<String>,

    /// File system paths with read permissions
    #[clap(long)]
    fs_read: Vec<String>,

    /// Enable network access
    #[clap(long)]
    network: bool,

    /// The command and arguments to execute in the sandbox
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

fn main() {
    // Initialize the sandbox trampoline - this checks if we're being called
    // with __sandbox_trampoline__ and if so, sets up the actual sandbox
    rattler_sandbox::init_sandbox();

    // Parse command line arguments (only reached if not in trampoline mode)
    let opt = Opt::parse();

    // Validate that a command was provided
    if opt.args.is_empty() {
        eprintln!("Error: No command provided");
        std::process::exit(1);
    }

    // Build the list of exceptions based on the command line options
    let mut exceptions = Vec::new();

    for path in opt.fs_exec_and_read {
        exceptions.push(Exception::ExecuteAndRead(path));
    }

    for path in opt.fs_write_and_read {
        exceptions.push(Exception::ReadAndWrite(path));
    }

    for path in opt.fs_read {
        exceptions.push(Exception::Read(path));
    }

    if opt.network {
        exceptions.push(Exception::Networking);
    }

    // Create a sandboxed command
    let mut command = sandboxed_command(&opt.args[0], &exceptions);

    // Add any additional arguments to the command
    command.args(&opt.args[1..]);

    // Execute the sandboxed command
    let status = command.status().expect("Failed to execute command");

    std::process::exit(status.code().unwrap_or(1));
}
