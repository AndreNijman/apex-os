//! This machine's long-term static keypair.
//!
//! One X25519 keypair per user, generated on first use and never rotated
//! automatically. It is the thing a device pins when it scans a QR code, so
//! rotating it unpairs every device — which is a decision the owner makes and
//! not something a daemon does on a schedule.
//!
//! ## Where the secret lives, and what protects it
//!
//! `$XDG_STATE_HOME/apex/remote/identity.key`, mode 0600, in a 0700
//! directory. That is the same protection `apex-agentd`'s control socket and
//! the privilege request store have, and it is the honest ceiling: this is a
//! per-user unprivileged service, so anything running as this user can read
//! it. A device key in a TPM would be better and is not available here —
//! nothing on any APEX machine currently verifies an image signature either,
//! and building on a hardware root of trust this platform has not got would
//! be a claim rather than a protection. See the design note.
//!
//! What it does buy: a *remote* attacker never sees it, because it never
//! travels. The handshake proves possession without transmitting it, which is
//! the whole reason the pairing is a key exchange rather than a shared
//! passphrase.
//!
//! ## Why the secret is never in a `Debug`, a listing or an error
//!
//! [`Identity`] deliberately implements neither `Serialize` nor `Debug`. The
//! same trick `apex-secret-core`'s `SecretValue` uses: a value that cannot be
//! serialised cannot be put in a reply by accident, and the compiler enforces
//! it rather than a review. [`Identity::secret_key_base64`] exists and is
//! `#[cfg(test)]`-adjacent in spirit — it is public only so a test can assert
//! the secret is absent from something, which needs the secret to compare
//! against.

use std::path::{Path, PathBuf};

/// A keypair, and the file it came from.
///
/// No `Debug`, no `Serialize`, no `Display`. Printing one is how a secret
/// ends up in a log line, and the way to make that impossible is to give it
/// no way to be printed.
pub struct Identity {
    secret: [u8; 32],
    public: [u8; 32],
}

/// Why an identity could not be loaded or made.
#[derive(Debug)]
pub enum IdentityError {
    Io(std::io::Error),
    /// The file exists and is not a key.
    ///
    /// Never repaired by generating a new one. A fresh key silently unpairs
    /// every device the owner has, and the symptom is "all my phones stopped
    /// working" with nothing pointing at the cause.
    Corrupt(String),
    /// The library would not produce a keypair.
    Crypto(String),
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdentityError::Io(e) => write!(f, "{e}"),
            IdentityError::Corrupt(w) => write!(
                f,
                "{w}. This machine's remote identity is what every paired device pinned, so it \
                 is not replaced automatically: move the file aside and re-pair if you meant to \
                 start again"
            ),
            IdentityError::Crypto(w) => write!(f, "{w}"),
        }
    }
}

impl std::error::Error for IdentityError {}

impl From<std::io::Error> for IdentityError {
    fn from(e: std::io::Error) -> IdentityError {
        IdentityError::Io(e)
    }
}

impl Identity {
    /// Where the key lives for this user.
    pub fn path() -> PathBuf {
        Self::path_in(&apex_agent_core::paths::state_home())
    }

    /// Where it lives under an arbitrary state root.
    pub fn path_in(state_home: &Path) -> PathBuf {
        state_home.join("apex").join("remote").join("identity.key")
    }

    /// A fresh keypair, from the Noise library's own generator.
    ///
    /// snow's `generate_keypair` goes to the operating system's CSPRNG
    /// through `getrandom`. Deliberately not `rand::random` on a `[u8; 32]`:
    /// that reads the *thread* RNG, which is seeded from the OS but is a
    /// userspace generator, and a long-term identity key should come from the
    /// same place the handshake's ephemerals do.
    pub fn generate() -> Result<Identity, IdentityError> {
        let kp = crate::noise::builder()
            .generate_keypair()
            .map_err(|e| IdentityError::Crypto(format!("generating a keypair: {e}")))?;
        let secret: [u8; 32] = kp
            .private
            .as_slice()
            .try_into()
            .map_err(|_| IdentityError::Crypto("the keypair is not 32 bytes".into()))?;
        let public: [u8; 32] = kp
            .public
            .as_slice()
            .try_into()
            .map_err(|_| IdentityError::Crypto("the keypair is not 32 bytes".into()))?;
        Ok(Identity { secret, public })
    }

    /// Load the key at `path`, generating and saving one if there is none.
    ///
    /// The generate-if-absent case writes with `create_new`, so two daemons
    /// racing on first start cannot both win: the loser gets `AlreadyExists`
    /// and reads the winner's key rather than overwriting it with its own.
    /// Overwriting would unpair every device paired in the last few
    /// milliseconds, which is a small window and an unrecoverable outcome.
    pub fn load_or_create(path: &Path) -> Result<Identity, IdentityError> {
        match Self::load(path) {
            Ok(id) => Ok(id),
            Err(IdentityError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                let id = Identity::generate()?;
                match id.save_new(path) {
                    Ok(()) => Ok(id),
                    Err(IdentityError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        Self::load(path)
                    }
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Read a key. A file that is not one is an error, never a fresh key.
    pub fn load(path: &Path) -> Result<Identity, IdentityError> {
        let text = std::fs::read_to_string(path)?;
        let secret = crate::b64_decode(text.trim()).ok_or_else(|| {
            IdentityError::Corrupt(format!("{} is not a base64url key", path.display()))
        })?;
        let secret: [u8; 32] = secret.as_slice().try_into().map_err(|_| {
            IdentityError::Corrupt(format!(
                "{} holds {} bytes; a key is 32",
                path.display(),
                secret.len()
            ))
        })?;
        let public = crate::noise::public_from_secret(&secret)
            .map_err(|e| IdentityError::Corrupt(format!("{}: {e}", path.display())))?;
        Ok(Identity { secret, public })
    }

    /// Write a key that must not already exist, 0600 in a 0700 directory.
    fn save_new(&self, path: &Path) -> Result<(), IdentityError> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let dir = path.parent().unwrap_or(Path::new("."));
        apex_agent_core::paths::ensure_private_dir(dir)?;
        // 0600 at creation, not afterwards. A `write` then `set_permissions`
        // leaves a window in which the key is world-readable, and the window
        // is exactly as long as the scheduler feels like making it.
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(crate::b64_encode(&self.secret).as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        Ok(())
    }

    /// The public key, base64url. Safe everywhere: it is what goes in the QR
    /// code.
    pub fn public_key(&self) -> String {
        crate::b64_encode(&self.public)
    }

    /// The raw public key.
    pub fn public_bytes(&self) -> [u8; 32] {
        self.public
    }

    /// The raw secret key, for handing to the Noise builder.
    ///
    /// Crate-internal on purpose: outside this crate there is no legitimate
    /// reason to hold one.
    pub(crate) fn secret_bytes(&self) -> [u8; 32] {
        self.secret
    }

    /// The secret key as text.
    ///
    /// Public for exactly one reason: a test that asserts the secret is
    /// absent from a serialised store needs the secret to compare against,
    /// and a test that compares against a hardcoded sentinel proves nothing
    /// about a real key. Nothing in the shipped binaries calls it, and
    /// `no_shipped_code_prints_the_secret_key` keeps that true.
    pub fn secret_key_base64(&self) -> String {
        crate::b64_encode(&self.secret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tmp(tag: &str) -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "apex-remote-id-{}-{tag}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        std::fs::create_dir_all(&p).expect("tmp");
        Tmp(p)
    }

    #[test]
    fn a_generated_key_is_stable_across_a_save_and_a_load() {
        let t = tmp("stable");
        let path = Identity::path_in(&t.0);
        let first = Identity::load_or_create(&path).expect("create");
        let second = Identity::load_or_create(&path).expect("load");
        assert_eq!(first.public_key(), second.public_key());
        assert_eq!(first.secret_key_base64(), second.secret_key_base64());
    }

    #[test]
    fn two_generated_keys_differ() {
        // A generator that returned a constant would pass every other test
        // here, and would give every APEX machine the same identity.
        let a = Identity::generate().expect("a");
        let b = Identity::generate().expect("b");
        assert_ne!(a.public_key(), b.public_key());
        assert_ne!(a.secret_key_base64(), b.secret_key_base64());
        // And not all zeroes, which is what an uninitialised buffer looks
        // like and what a broken RNG most often returns.
        assert_ne!(a.public_bytes(), [0u8; 32]);
        assert_ne!(a.secret_bytes(), [0u8; 32]);
    }

    #[test]
    fn the_public_key_is_derived_from_the_secret_and_not_stored_beside_it() {
        // The file holds one value. If the public key were stored too, a file
        // edited to change it would produce an identity whose halves do not
        // match, and every handshake would fail for a reason nothing explains.
        let t = tmp("derive");
        let path = Identity::path_in(&t.0);
        let id = Identity::load_or_create(&path).expect("create");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(!text.contains(&id.public_key()), "the file holds the public key too");
        assert!(text.trim() == id.secret_key_base64());
        assert_eq!(
            crate::noise::public_from_secret(&id.secret_bytes()).expect("derive"),
            id.public_bytes()
        );
    }

    #[test]
    fn the_key_file_is_created_owner_only_with_no_readable_window() {
        use std::os::unix::fs::PermissionsExt;
        let t = tmp("mode");
        let path = Identity::path_in(&t.0);
        Identity::load_or_create(&path).expect("create");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the identity key is readable by others");
        let dir = std::fs::metadata(path.parent().expect("dir"))
            .expect("stat")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir, 0o700);
    }

    #[test]
    fn a_corrupt_key_file_is_an_error_and_not_a_fresh_identity() {
        // The failure this refuses to cause: a new key unpairs every device,
        // and nothing about "my phone stopped working" points at the file.
        let t = tmp("corrupt");
        let path = Identity::path_in(&t.0);
        std::fs::create_dir_all(path.parent().expect("dir")).expect("mkdir");
        for bad in ["", "not base64!!", &crate::b64_encode(&[0u8; 16])] {
            std::fs::write(&path, bad).expect("write");
            // `expect_err` is unavailable here and that is the point:
            // `Identity` has no `Debug`, so it cannot be printed by a test
            // helper, a panic message or a stray `dbg!`. Matched instead.
            match Identity::load_or_create(&path) {
                Err(IdentityError::Corrupt(_)) => {}
                Err(other) => panic!("{bad:?}: wrong error {other}"),
                Ok(_) => panic!("{bad:?}: a corrupt key file was replaced"),
            }
            // And the file is untouched.
            assert_eq!(std::fs::read_to_string(&path).expect("read"), bad);
        }
    }

    #[test]
    fn creating_twice_at_once_keeps_one_key() {
        // Two daemons starting together. `create_new` makes one of them lose,
        // and the loser must adopt the winner's key rather than overwrite it —
        // an overwrite unpairs whatever paired in between.
        let t = tmp("race");
        let path = Identity::path_in(&t.0);
        let first = Identity::generate().expect("generate");
        first.save_new(&path).expect("save");
        let second = Identity::generate().expect("generate");
        match second.save_new(&path) {
            Err(IdentityError::Io(io)) if io.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(other) => panic!("wrong error: {other}"),
            Ok(()) => panic!("the second key overwrote the first"),
        }
        assert_eq!(
            Identity::load_or_create(&path).expect("load").public_key(),
            first.public_key()
        );
    }
}
