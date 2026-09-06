//! Local Inter-Process Communication (IPC) layer for MBHub.
//!
//! Enables headless clients (`mbhub ask`, MCP server) to communicate
//! seamlessly with a long-running background `mbhub daemon`.
//!
//! Unix: Unix Domain Socket (`~/.mbhub/mbhub.sock`). Access is gated by file
//! permissions: the containing `.mbhub` directory is locked to 0700 BEFORE
//! the socket file is created (closing the bind→chmod TOCTOU window, audit
//! O9) and the socket file itself is chmod'ed to 0600 after bind.
//!
//! Windows: Local loopback TCP (`127.0.0.1:45550`). SECURITY NOTE: this
//! transport performs NO authentication — loopback-only binding is the only
//! isolation, so on multi-user Windows hosts any local user/process can
//! connect and issue IPC requests. Documented limitation (audit O9); closing
//! it requires a shared-secret or named-pipe + ACL design.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum IpcRequest {
    Ping,
    Ask { query: String },
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum IpcResponse {
    Pong,
    Answer {
        question: String,
        content: String,
        source: String,
        similarity: f64,
        is_swarm: bool,
    },
    Status {
        running: bool,
        peers: usize,
        reserved_gb: u64,
        records: usize,
    },
    Error(String),
}

/// Returns the path to the Unix domain socket for MBHub daemon.
///
/// Override with `MBHUB_SOCK`. The default location lives under the
/// user-private `~/.mbhub/` directory; `ServerListener::bind` locks that
/// directory to 0700 before the socket file is created. When overriding,
/// point `MBHUB_SOCK` into a directory only the current user can traverse —
/// MBHub never chmods directories outside its own `.mbhub` state directory.
pub fn socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("MBHUB_SOCK") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".mbhub").join("mbhub.sock")
}

/// Unix-only: owner-only hardening of the socket's containing directory,
/// applied BEFORE the socket inode exists.
///
/// `bind()` creates the socket file with umask-derived (potentially
/// world-accessible) permissions and can only be chmod'ed afterwards — a
/// short TOCTOU window in which other local users could reach the socket
/// (audit O9). The gate is moved one level up instead: the containing
/// `.mbhub` directory is created with mode 0700 directly (no create→chmod
/// window) or chmod'ed to 0700 when it already exists.
///
/// Deliberately narrow: only directories named `.mbhub` are ever touched, so
/// an `MBHUB_SOCK` override pointing somewhere else (e.g. `/tmp`) can never
/// cause MBHub to chmod an unrelated directory.
#[cfg(unix)]
fn harden_socket_location(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::fs::PermissionsExt;

    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.file_name() != Some(std::ffi::OsStr::new(".mbhub")) {
        return Ok(());
    }

    // Create all missing levels directly with the final mode.
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    builder.mode(0o700);
    builder.create(parent)?;

    // A pre-existing `.mbhub` (created earlier under a lax umask) is locked
    // down as well.
    let meta = std::fs::metadata(parent)?;
    if meta.is_dir() {
        let mut perms = meta.permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(parent, perms)?;
    }
    Ok(())
}

#[cfg(unix)]
mod transport {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::time::Duration;

    pub struct ServerListener(UnixListener);

    impl ServerListener {
        pub fn bind() -> std::io::Result<Self> {
            let path = socket_path();
            // Close the bind→chmod TOCTOU window (audit O9): for the default
            // location the containing `.mbhub` directory is created with (or
            // locked to) mode 0700 BEFORE the socket inode exists.
            let _ = harden_socket_location(&path);
            if let Some(parent) = path.parent() {
                // Non-`.mbhub` override locations still get their directory
                // (best effort, historical behavior).
                let _ = std::fs::create_dir_all(parent);
            }
            // Remove stale socket file if daemon crashed
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
            let listener = UnixListener::bind(&path)?;
            // Secure socket file permissions (0600 on Unix). Defense in
            // depth on top of the directory gate above.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
            Ok(Self(listener))
        }

        pub fn accept(&self) -> std::io::Result<Stream> {
            let (stream, _) = self.0.accept()?;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
            Ok(Stream(stream))
        }
    }

    impl Drop for ServerListener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(socket_path());
        }
    }

    pub struct Stream(UnixStream);

    impl Stream {
        pub fn connect() -> std::io::Result<Self> {
            let path = socket_path();
            let stream = UnixStream::connect(path)?;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
            Ok(Self(stream))
        }

        pub fn send_request(&mut self, req: &IpcRequest) -> std::io::Result<IpcResponse> {
            let mut line = serde_json::to_string(req)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            line.push('\n');
            self.0.write_all(line.as_bytes())?;
            self.0.flush()?;

            let mut reader = BufReader::new(&mut self.0);
            let mut resp_line = String::new();
            reader.read_line(&mut resp_line)?;

            let resp: IpcResponse = serde_json::from_str(resp_line.trim())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            Ok(resp)
        }

        pub fn read_request(&mut self) -> std::io::Result<IpcRequest> {
            let mut reader = BufReader::new(&mut self.0);
            let mut line = String::new();
            reader.read_line(&mut line)?;
            if line.trim().is_empty() {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "empty line"));
            }
            let req: IpcRequest = serde_json::from_str(line.trim())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            Ok(req)
        }

        pub fn write_response(&mut self, resp: &IpcResponse) -> std::io::Result<()> {
            let mut line = serde_json::to_string(resp)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            line.push('\n');
            self.0.write_all(line.as_bytes())?;
            self.0.flush()?;
            Ok(())
        }
    }
}

#[cfg(not(unix))]
mod transport {
    //! Windows loopback TCP transport.
    //!
    //! SECURITY NOTE (audit O9): this transport performs **no authentication**.
    //! Binding to `127.0.0.1` only restricts the listener to loopback — any
    //! local process, and on multi-user hosts any interactive user session,
    //! can connect to the port and issue IPC requests (an `Ask` consumes the
    //! user's paid API budget; `Status` reads local metadata). Documented
    //! limitation, no behavior change is intended here; closing it requires a
    //! shared-secret or named-pipe + ACL design.

    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    const TCP_PORT: u16 = 45550;

    pub struct ServerListener(TcpListener);

    impl ServerListener {
        pub fn bind() -> std::io::Result<Self> {
            let listener = TcpListener::bind(("127.0.0.1", TCP_PORT))?;
            Ok(Self(listener))
        }

        pub fn accept(&self) -> std::io::Result<Stream> {
            let (stream, _) = self.0.accept()?;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
            Ok(Stream(stream))
        }
    }

    pub struct Stream(TcpStream);

    impl Stream {
        pub fn connect() -> std::io::Result<Self> {
            let stream = TcpStream::connect(("127.0.0.1", TCP_PORT))?;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
            Ok(Self(stream))
        }

        pub fn send_request(&mut self, req: &IpcRequest) -> std::io::Result<IpcResponse> {
            let mut line = serde_json::to_string(req)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            line.push('\n');
            self.0.write_all(line.as_bytes())?;
            self.0.flush()?;

            let mut reader = BufReader::new(&mut self.0);
            let mut resp_line = String::new();
            reader.read_line(&mut resp_line)?;

            let resp: IpcResponse = serde_json::from_str(resp_line.trim())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            Ok(resp)
        }

        pub fn read_request(&mut self) -> std::io::Result<IpcRequest> {
            let mut reader = BufReader::new(&mut self.0);
            let mut line = String::new();
            reader.read_line(&mut line)?;
            if line.trim().is_empty() {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "empty line"));
            }
            let req: IpcRequest = serde_json::from_str(line.trim())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            Ok(req)
        }

        pub fn write_response(&mut self, resp: &IpcResponse) -> std::io::Result<()> {
            let mut line = serde_json::to_string(resp)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            line.push('\n');
            self.0.write_all(line.as_bytes())?;
            self.0.flush()?;
            Ok(())
        }
    }
}

pub use transport::{ServerListener, Stream};

/// Tries to send an IPC request to an already-running daemon.
/// Returns None if daemon is not running or socket is unreachable.
pub fn try_query_daemon(req: &IpcRequest) -> Option<IpcResponse> {
    if let Ok(mut stream) = Stream::connect() {
        stream.send_request(req).ok()
    } else {
        None
    }
}

/// Serializes env-mutating IPC tests (`MBHUB_SOCK` is process-global and
/// must not race with other test modules).
#[cfg(test)]
#[cfg(unix)]
pub static IPC_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_request_response_serialization_round_trip() {
        let req = IpcRequest::Ask {
            query: "What is quantum entanglement?".to_string(),
        };
        let serialized = serde_json::to_string(&req).expect("serialize req");
        let deserialized: IpcRequest = serde_json::from_str(&serialized).expect("deserialize req");
        match deserialized {
            IpcRequest::Ask { query } => assert_eq!(query, "What is quantum entanglement?"),
            _ => panic!("wrong request variant"),
        }

        let resp = IpcResponse::Answer {
            question: "What is quantum entanglement?".to_string(),
            content: "Entanglement is a quantum phenomenon...".to_string(),
            source: "L1 (local SQLite)".to_string(),
            similarity: 98.5,
            is_swarm: false,
        };
        let serialized_resp = serde_json::to_string(&resp).expect("serialize resp");
        let deserialized_resp: IpcResponse =
            serde_json::from_str(&serialized_resp).expect("deserialize resp");
        match deserialized_resp {
            IpcResponse::Answer {
                question,
                content,
                source,
                similarity,
                is_swarm,
            } => {
                assert_eq!(question, "What is quantum entanglement?");
                assert_eq!(content, "Entanglement is a quantum phenomenon...");
                assert_eq!(source, "L1 (local SQLite)");
                assert!((similarity - 98.5).abs() < 0.001);
                assert!(!is_swarm);
            }
            _ => panic!("wrong response variant"),
        }
    }

    /// Asserts the permission bits of `path` (unix only; called from unix-only tests).
    #[cfg(unix)]
    fn assert_mode(path: &std::path::Path, expected: u32, what: &str) {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)
            .unwrap_or_else(|e| panic!("{what}: metadata failed: {e}"))
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            expected,
            "{what}: expected mode {expected:o}, got {mode:o}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn socket_location_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        // Audit O9: the containing `.mbhub` directory must end up at 0700 —
        // created with that mode when missing, locked down when pre-existing.
        let base = std::env::temp_dir().join(format!("mbhub_ipc_sock_{}", std::process::id()));
        let dir = base.join(".mbhub");
        let sock = dir.join("mbhub.sock");
        let _ = std::fs::remove_dir_all(&base);

        // 1. Missing directory: created directly with 0700.
        harden_socket_location(&sock).expect("harden missing location");
        assert!(dir.is_dir(), "directory must be created");
        assert_mode(&dir, 0o700, "freshly created .mbhub");

        // 2. Pre-existing lax directory: locked down to 0700.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        harden_socket_location(&sock).expect("harden existing location");
        assert_mode(&dir, 0o700, "pre-existing .mbhub");

        // 3. Non-`.mbhub` locations are never touched: an MBHUB_SOCK override
        //    may point anywhere (e.g. /tmp) and MBHub must not chmod
        //    unrelated directories.
        let plain = base.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = harden_socket_location(&plain.join("mbhub.sock"));
        assert_mode(&plain, 0o755, "non-.mbhub directory must stay untouched");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn daemon_socket_and_directory_permissions_are_owner_only() {
        // Audit O9 end-to-end: bind() must leave the socket file at 0600 and
        // its containing `.mbhub` directory at 0700, and the socket must stay
        // fully functional. Redirected via MBHUB_SOCK (the env redirection
        // pattern used by this crate's tests).
        let _guard = IPC_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("mbhub_ipc_bind_{}", std::process::id()));
        let sock = base.join(".mbhub").join("mbhub.sock");
        let _ = std::fs::remove_dir_all(&base);
        unsafe {
            std::env::set_var("MBHUB_SOCK", &sock);
        }

        let listener = ServerListener::bind().expect("bind daemon socket");
        let dir = sock.parent().expect("socket parent").to_path_buf();

        assert_mode(&sock, 0o600, "socket file");
        assert_mode(&dir, 0o700, "socket directory");

        // Behavior preservation: a full Ping round-trip over the bound socket.
        let server = std::thread::spawn(move || {
            let mut conn = listener.accept().expect("accept");
            let req = conn.read_request().expect("read request");
            assert!(matches!(req, IpcRequest::Ping));
            conn.write_response(&IpcResponse::Pong).expect("write response");
        });
        let mut client = Stream::connect().expect("client connect");
        let resp = client.send_request(&IpcRequest::Ping).expect("ping");
        assert!(matches!(resp, IpcResponse::Pong));
        server.join().expect("server thread");
        // The listener was dropped at the end of the server closure: its Drop
        // handler must have removed the socket file.
        assert!(
            !sock.exists(),
            "socket file must be removed when the listener is dropped"
        );

        unsafe {
            std::env::remove_var("MBHUB_SOCK");
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
