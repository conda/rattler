use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use async_trait::async_trait;

use crate::{
    backends::nfs_fs::NfsFS,
    mount::{MountProvider, MountSession},
    virtual_fs_core::VirtualFSCore,
};

use nfs3_server::tcp::{NFSTcp, NFSTcpListener};

const NFS_ADDR: &str = "127.0.0.1:11111";
#[cfg(target_os = "linux")]
const NFS_PORT: u16 = 11111;

pub struct NfsProvider;

pub struct NfsSession {
    mount_point: PathBuf,
    server_thread: std::thread::JoinHandle<anyhow::Result<()>>,
}

impl MountSession for NfsSession {
    fn unmount(self: Box<Self>) -> anyhow::Result<()> {
        let status = std::process::Command::new("umount")
            .arg(&self.mount_point)
            .status()
            .context("failed to run umount")?;

        if !status.success() {
            eprintln!(
                "umount exited with {:?} — server still stopping",
                status.code()
            );
        }

        #[cfg(target_os = "linux")]
        rpcbind_unregister(NFS_PORT);

        drop(self.server_thread);
        Ok(())
    }
}

#[async_trait]
impl MountProvider for NfsProvider {
    async fn mount(
        fs: Arc<VirtualFSCore>,
        mount_point: PathBuf,
    ) -> anyhow::Result<Box<dyn MountSession>> {
        let filesystem = NfsFS { inner: fs };
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();

        let server_thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;

            runtime.block_on(async move {
                let listener = match NFSTcpListener::bind(NFS_ADDR, filesystem).await {
                    Ok(listener) => {
                        let _ = ready_tx.send(Ok(()));
                        listener
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err.to_string()));
                        return Err(err.into());
                    }
                };

                listener.handle_forever().await.map_err(Into::into)
            })
        });

        ready_rx
            .recv()
            .map_err(|err| anyhow::anyhow!("NFS server thread exited before binding: {err}"))?
            .map_err(|err| anyhow::anyhow!("failed binding NFS listener: {err}"))?;

        eprintln!("NFS server listening on {NFS_ADDR}");

        #[cfg(target_os = "macos")]
        let status = std::process::Command::new("mount_nfs")
            .args([
                "-o",
                "vers=3,tcp,port=11111,mountport=11111,nolockd,nolock,soft,intr,noresvport",
            ])
            .arg("127.0.0.1:/")
            .arg(&mount_point)
            .status()
            .context("failed to run mount_nfs — try running with sudo")?;

        #[cfg(target_os = "linux")]
        {
            // Verify the server we just started is actually reachable.
            std::net::TcpStream::connect("127.0.0.1:11111")
                .context("NFS server not reachable on 127.0.0.1:11111 immediately after start")?;

            // Tell the system portmapper (if running) about our server so the
            // kernel NFS client can discover it on port 11111.
            let rpcbind_running = rpcbind_register(NFS_PORT);
            eprintln!(
                "rpcbind registration: {}",
                if rpcbind_running {
                    "ok"
                } else {
                    "skipped (rpcbind not running)"
                }
            );

            linux_nfs_mount(&mount_point)?;
        }

        #[cfg(not(target_os = "linux"))]
        if !status.success() {
            return Err(anyhow::anyhow!(
                "mount command failed with exit code {:?}",
                status.code()
            ));
        }

        eprintln!("mounted at {}", mount_point.display());

        Ok(Box::new(NfsSession {
            mount_point,
            server_thread,
        }))
    }
}

/// Mount the NFS server at 127.0.0.1:11111 (registered with rpcbind).
///
/// mount(2) / mount.nfs require CAP_SYS_ADMIN. We try, in order:
///   1. `sudo -n mount.nfs`  — passwordless-sudo path
///   2. `mount.nfs`          — direct (works if the process is already root)
///   3. `mount -t nfs`       — fallback for distros without mount.nfs in PATH
///
/// If every attempt fails the error includes a hint about privilege requirements.
#[cfg(target_os = "linux")]
fn linux_nfs_mount(mount_point: &std::path::Path) -> anyhow::Result<()> {
    // With rpcbind running and our programs registered on port 11111, mount.nfs
    // discovers the ports via portmapper in userspace (binary-mode mount), so
    // we do NOT specify port= or mountport= here.
    let opts = "nolock,nfsvers=3,tcp";
    let source = "127.0.0.1:/";
    let target = mount_point;

    // Try each candidate in order, stop at the first one that runs (even if it
    // fails — we want that specific failure, not an exec-not-found error).
    let candidates: &[&[&str]] = &[
        &["sudo", "-n", "mount.nfs", "-o", opts, source],
        &["mount.nfs", "-o", opts, source],
        &["mount", "-t", "nfs", "-o", opts, source],
    ];

    let mut last_err = String::new();
    for argv in candidates {
        let Ok(output) = std::process::Command::new(argv[0])
            .args(&argv[1..])
            .arg(target)
            .output()
        else {
            continue; // binary not found / not executable
        };

        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        last_err = format!(
            "{} (exit {:?}): stdout={} stderr={}",
            argv[0],
            output.status.code(),
            stdout.trim(),
            stderr.trim()
        );

        // "failed to apply fstab options" = privilege check inside mount.nfs.
        // Try the next candidate (sudo might succeed where non-sudo fails).
        // For any other error the mount itself is failing; stop trying.
        let combined = format!("{stdout}{stderr}");
        if !combined.contains("fstab") {
            break;
        }
    }

    Err(anyhow::anyhow!(
        "{last_err}\n\nHint: NFS mounting on Linux requires CAP_SYS_ADMIN. \
         Run the process as root, or add a passwordless sudo rule:\n  \
         %user ALL=(root) NOPASSWD: /sbin/mount.nfs"
    ))
}

/// Register NFS3 (100003) and MOUNT (100005) with system rpcbind on port 111.
/// Returns true if rpcbind was reached, false if it is not running.
#[cfg(target_os = "linux")]
fn rpcbind_register(port: u16) -> bool {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let Ok(mut stream) =
        TcpStream::connect_timeout(&"127.0.0.1:111".parse().unwrap(), Duration::from_secs(1))
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

    for prog in [100003u32, 100005] {
        let msg = pmap_request(1, prog, 3, port);
        if stream.write_all(&msg).is_err() {
            return false;
        }
        let mut hdr = [0u8; 4];
        if stream.read_exact(&mut hdr).is_err() {
            return false;
        }
        let body_len = (u32::from_be_bytes(hdr) & 0x7FFF_FFFF) as usize;
        let mut body = vec![0u8; body_len];
        let _ = stream.read_exact(&mut body);
    }
    true
}

#[cfg(target_os = "linux")]
fn rpcbind_unregister(port: u16) {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let Ok(mut stream) =
        TcpStream::connect_timeout(&"127.0.0.1:111".parse().unwrap(), Duration::from_secs(1))
    else {
        return;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

    for prog in [100003u32, 100005] {
        let msg = pmap_request(2, prog, 3, port);
        if stream.write_all(&msg).is_err() {
            return;
        }
        let mut hdr = [0u8; 4];
        if stream.read_exact(&mut hdr).is_err() {
            return;
        }
        let body_len = (u32::from_be_bytes(hdr) & 0x7FFF_FFFF) as usize;
        let mut body = vec![0u8; body_len];
        let _ = stream.read_exact(&mut body);
    }
}

/// Build a TCP-framed portmapper RPC call (PMAPPROC_SET=1 or PMAPPROC_UNSET=2).
#[cfg(target_os = "linux")]
fn pmap_request(proc: u32, prog: u32, vers: u32, port: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(56);
    let mut w = |n: u32| payload.extend_from_slice(&n.to_be_bytes());

    w(prog);
    w(0);
    w(2);
    w(100_000);
    w(2);
    w(proc);
    w(0);
    w(0); // cred: AUTH_NULL
    w(0);
    w(0); // verf: AUTH_NULL
    w(prog);
    w(vers);
    w(6);
    w(port as u32); // mapping: prog, vers, IPPROTO_TCP, port

    let mut msg = Vec::with_capacity(4 + payload.len());
    msg.extend_from_slice(&((payload.len() as u32) | 0x8000_0000).to_be_bytes());
    msg.extend_from_slice(&payload);
    msg
}
