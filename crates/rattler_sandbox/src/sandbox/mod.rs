use birdcage::process::Command;
use birdcage::{Birdcage, Sandbox};
use clap::Parser;

pub mod sandbox_impl;
#[cfg(feature = "tokio")]
pub mod tokio;

pub use sandbox_impl::{sandboxed_command, Exception};

#[derive(clap::Parser)]
pub struct Opts {
    #[clap(long)]
    pub fs_exec_and_read: Option<Vec<String>>,

    #[clap(long)]
    pub fs_write_and_read: Option<Vec<String>>,

    #[clap(long)]
    pub fs_read: Option<Vec<String>>,

    #[clap(long)]
    pub network: bool,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

// This function checks if the current executable should execute as a sandboxed process
fn init() {
    let mut args = std::env::args().collect::<Vec<String>>();
    // Remove the first `__sandbox_trampoline__` argument
    args.remove(1);
    let opts = Opts::parse_from(args.iter());
    // Allow access to our test executable.
    let mut sandbox = Birdcage::new();

    if let Some(fs_exec_and_read) = opts.fs_exec_and_read {
        for path in fs_exec_and_read {
            let _ = sandbox.add_exception(birdcage::Exception::ExecuteAndRead(path.into()));
        }
    }

    if let Some(fs_read) = opts.fs_read {
        for path in fs_read {
            let _ = sandbox.add_exception(birdcage::Exception::Read(path.into()));
        }
    }

    if let Some(fs_write_and_read) = opts.fs_write_and_read {
        for path in fs_write_and_read {
            let _ = sandbox.add_exception(birdcage::Exception::WriteAndRead(path.into()));
        }
    }

    if opts.network {
        let _ = sandbox.add_exception(birdcage::Exception::Networking);
    }

    if let Some((exe, args)) = opts.args.split_first() {
        // Initialize the sandbox; by default everything is prohibited.
        let mut command = Command::new(exe);
        command.args(args);

        let mut child = sandbox.spawn(command).unwrap();

        let status = child.wait().unwrap();
        std::process::exit(status.code().unwrap());
    } else {
        panic!("No executable provided");
    }
}

pub fn init_sandbox() {
    // TODO ideally we check that we are single threaded, but birdcage will also check it later on
    if std::env::args().any(|arg| arg == "__sandbox_trampoline__") {
        // This is a sandboxed process, so we initialize the sandbox
        init();
    }
}
