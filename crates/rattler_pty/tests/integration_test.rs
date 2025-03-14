#[cfg(test)]
#[cfg(unix)]
mod tests {

    use rattler_pty::unix::PtyProcess;
    use rattler_pty::unix::PtyProcessOptions;

    use nix::sys::{signal, wait};
    use std::io::{BufRead, BufReader, LineWriter, Write};
    use std::{self, process::Command, thread, time};

    #[test]
    /// Open cat, write string, read back string twice, send Ctrl^C and check that cat exited
    fn test_cat() -> std::io::Result<()> {
        let process = PtyProcess::new(
            Command::new("cat"),
            PtyProcessOptions {
                echo: false,
                window_size: Option::default(),
            },
        )
        .expect("could not execute cat");
        let f = process.get_file_handle().unwrap();
        let mut writer = LineWriter::new(&f);
        let mut reader = BufReader::new(&f);
        let _ = writer.write(b"hello cat\n")?;
        let mut buf = String::new();
        reader.read_line(&mut buf)?;
        assert_eq!(buf, "hello cat\r\n");

        // this sleep solves an edge case of some cases when cat is somehow not "ready"
        // to take the ^C (occasional test hangs)
        thread::sleep(time::Duration::from_millis(100));
        writer.write_all(&[3])?; // send ^C
        writer.flush()?;
        let should = wait::WaitStatus::Signaled(process.child_pid, signal::Signal::SIGINT, false);
        assert_eq!(should, wait::waitpid(process.child_pid, None).unwrap());
        Ok(())
    }
}
