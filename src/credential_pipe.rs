//! Explicit private handoff from the workstation Infisical fork.
//! Token bytes never travel on either public output stream.

use std::os::fd::AsRawFd;
use std::process::{Output, Stdio};
use tokio::io::AsyncReadExt;

pub async fn capture_session() -> std::io::Result<(Output, Vec<u8>)> {
    let (writer, reader) = tokio::net::unix::pipe::pipe()?;
    let writer = writer.into_blocking_fd()?;
    let descriptor = writer.as_raw_fd();
    let mut command = tokio::process::Command::new("infisical");
    command
        .args([
            "user",
            "get",
            "token",
            "--plain",
            "--silent",
            "--output-fd=3",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Only async-signal-safe descriptor operations execute in the forked child.
    // The parent owns writer until spawn completes, then closes its copy.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(descriptor, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    drop(writer);
    let mut bytes = Vec::new();
    let mut bounded = reader.take(8193);
    let (output, read) = tokio::join!(child.wait_with_output(), bounded.read_to_end(&mut bytes));
    read?;
    if bytes.len() > 8192 {
        return Err(std::io::Error::other(
            "credential handoff exceeded its size limit",
        ));
    }
    Ok((output?, bytes))
}
