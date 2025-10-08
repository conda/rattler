#![cfg(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
))]
fn main() {
    use clap::Parser;
    use rattler_sandbox::{sandboxed_command, Exception, Opts};

    // Initialize the sandbox trampoline - this checks if we're being called
    // with __sandbox_trampoline__ and if so, sets up the actual sandbox.
    // If we're in trampoline mode, this function will handle everything and exit.
    rattler_sandbox::init_sandbox();

    // Parse command line arguments (only reached if not in trampoline mode)
    let opt = Opts::parse();

    // Validate that a command was provided
    if opt.args.is_empty() {
        eprintln!("Error: No command provided");
        std::process::exit(1);
    }

    // Build the list of exceptions based on the command line options
    let mut exceptions = Vec::new();

    if let Some(fs_exec_and_read) = opt.fs_exec_and_read {
        for path in fs_exec_and_read {
            exceptions.push(Exception::ExecuteAndRead(path));
        }
    }

    if let Some(fs_write_and_read) = opt.fs_write_and_read {
        for path in fs_write_and_read {
            exceptions.push(Exception::ReadAndWrite(path));
        }
    }

    if let Some(fs_read) = opt.fs_read {
        for path in fs_read {
            exceptions.push(Exception::Read(path));
        }
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

#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
)))]
fn main() {
    eprintln!("rattler-sandbox is not supported on this platform");
    std::process::exit(1);
}
