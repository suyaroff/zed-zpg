//! SSH port forwarding through the system `ssh`, so `~/.ssh/config`, keys,
//! the agent, `ProxyJump` and `known_hosts` all apply.

use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// What the tunnel reaches on the SSH server's side.
pub enum Remote {
    Tcp(String, u16),
    /// A Postgres unix socket file, e.g. `/var/run/postgresql/.s.PGSQL.5432`.
    Unix(String),
}

pub struct Tunnel {
    child: Child,
    pub port: u16,
}

impl Tunnel {
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> Result<u16, String> {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| format!("no free local port: {e}"))?;
    Ok(l.local_addr().map_err(|e| e.to_string())?.port())
}

/// Starts `ssh -L` to `target` and waits until the local end accepts.
pub fn open(target: &str, options: &[String], remote: &Remote) -> Result<Tunnel, String> {
    let port = free_port()?;
    let forward = match remote {
        Remote::Tcp(host, p) => format!("127.0.0.1:{port}:{host}:{p}"),
        Remote::Unix(path) => format!("127.0.0.1:{port}:{path}"),
    };
    let mut cmd = Command::new("ssh");
    cmd.args(["-N", "-L", &forward])
        // Never wait for a password or a host key prompt nobody can answer.
        .args(["-o", "BatchMode=yes", "-o", "ExitOnForwardFailure=yes"])
        .args(["-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=30"])
        .args(options)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // Take the tunnel down with zpg even if zpg is killed.
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
    }
    let mut child = cmd.spawn().map_err(|e| format!("failed to run ssh: {e}"))?;

    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let mut err = String::new();
            if let Some(mut s) = child.stderr.take() {
                let _ = s.read_to_string(&mut err);
            }
            let err = err.trim();
            return Err(if err.is_empty() {
                format!("ssh to {target} exited with {status}")
            } else {
                format!("ssh to {target}: {err}")
            });
        }
        // ssh opens the local end only once the session is up.
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            // Keep reading stderr so a chatty ssh never blocks on a full pipe.
            if let Some(mut s) = child.stderr.take() {
                std::thread::spawn(move || std::io::copy(&mut s, &mut std::io::sink()));
            }
            return Ok(Tunnel { child, port });
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("ssh to {target}: no tunnel after 20 s"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
