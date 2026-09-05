//! Local Inter-Process Communication (IPC) layer for MBHub.
//!
//! Enables headless clients (`mbhub ask`, MCP server) to communicate
//! seamlessly with a long-running background `mbhub daemon`.
//!
//! Unix: Unix Domain Socket (`~/.mbhub/mbhub.sock`)
//! Windows: Local loopback TCP (`127.0.0.1:45550`)

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
pub fn socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("MBHUB_SOCK") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".mbhub").join("mbhub.sock")
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
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            // Remove stale socket file if daemon crashed
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
            let listener = UnixListener::bind(&path)?;
            // Secure socket permissions (0600 on Unix)
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
}
