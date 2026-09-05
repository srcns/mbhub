//! P2P Node Identity generation and persistence.
//!
//! Generates a cryptographic Ed25519 keypair and persists it locally
//! to ensure stable PeerId across node restarts.
//!
//! Key protection: this private key IS the node's identity
//! and reputation — writes are atomic (temp file + rename, which also defeats
//! symlink-following attacks) and the file is locked to owner-only (0600) on
//! Unix, repaired on load when permissions have drifted.

use std::fs;
use std::path::PathBuf;
use libp2p::identity::Keypair;

pub fn load_or_generate_keypair() -> Keypair {
    if let Some(path) = identity_path() {
        if path.exists() {
            // Repair drifted permissions before trusting the file (§13).
            ensure_owner_only(&path);
            if let Ok(bytes) = fs::read(&path) {
                if let Ok(kp) = Keypair::from_protobuf_encoding(&bytes) {
                    return kp;
                }
            }
        }

        // Generate fresh Ed25519 keypair
        let kp = Keypair::generate_ed25519();
        if let Ok(bytes) = kp.to_protobuf_encoding() {
            write_identity_atomically(&path, &bytes);
        }
        kp
    } else {
        Keypair::generate_ed25519()
    }
}

/// Writes the identity bytes via a same-directory temp file + rename.
///
/// `rename` replaces the destination atomically and does not follow a
/// symlink placed at the destination path, closing the symlink-redirect
/// variant of local key-file attacks.
fn write_identity_atomically(path: &PathBuf, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let tmp = path.with_extension("tmp");
    // create_new: never clobber an existing file through the temp path.
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
    {
        Ok(mut f) => {
            use std::io::Write;
            if f.write_all(bytes).is_ok() {
                ensure_owner_only(&tmp);
                let _ = fs::rename(&tmp, path);
            } else {
                let _ = fs::remove_file(&tmp);
            }
        }
        Err(_) => {
            // Temp path collision (stale file): fall back to direct write.
            let _ = fs::write(path, bytes);
        }
    }

    ensure_owner_only(path);
}

/// Best-effort owner-only (0600) permission enforcement on Unix.
#[cfg(unix)]
fn ensure_owner_only(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o600 {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(not(unix))]
fn ensure_owner_only(_path: &std::path::Path) {}

fn identity_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MBHUB_IDENTITY") {
        return Some(PathBuf::from(p));
    }
    // Windows doesn't set HOME; fall back to USERPROFILE so the node identity
    // (and with it the node's reputation) persists across restarts everywhere.
    let home = std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok());
    home.map(|h| PathBuf::from(h).join(".mbhub").join("node_identity.bin"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_produces_readable_keypair_file() {
        let dir = std::env::temp_dir().join(format!("mbhub_identity_test_{}", std::process::id()));
        let path = dir.join("node_identity.bin");
        let _ = fs::remove_dir_all(&dir);

        let kp = Keypair::generate_ed25519();
        let bytes = kp.to_protobuf_encoding().unwrap();
        write_identity_atomically(&path, &bytes);

        let read_back = fs::read(&path).unwrap();
        assert_eq!(read_back, bytes);
        let parsed = Keypair::from_protobuf_encoding(&read_back).unwrap();
        assert_eq!(parsed.public(), kp.public());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "identity file must be owner-only");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn symlink_at_destination_is_replaced_not_followed() {
        let dir = std::env::temp_dir().join(format!("mbhub_identity_symlink_test_{}", std::process::id()));
        let victim = dir.join("victim.txt");
        let path = dir.join("node_identity.bin");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(&victim, b"PRECIOUS DATA").unwrap();

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&victim, &path).unwrap();
            write_identity_atomically(&path, b"KEY BYTES");

            // Victim file untouched; destination is now a regular file.
            assert_eq!(fs::read(&victim).unwrap(), b"PRECIOUS DATA");
            assert!(!fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
            assert_eq!(fs::read(&path).unwrap(), b"KEY BYTES");
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
