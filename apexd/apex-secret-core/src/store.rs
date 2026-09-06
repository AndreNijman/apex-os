//! The credential store: values in, metadata out, values never out.
//!
//! Two files per credential, and the split is the design rather than tidiness:
//!
//! * `<name>.json` — [`SecretMeta`], which has no field that could hold a value
//!   and derives `Serialize`, so it is safe to hand to a caller wholesale;
//! * `<name>.blob` — the sealed bytes, read only by [`open`], which is called
//!   from one place in the daemon and returns a [`SecretValue`] that cannot be
//!   serialised.
//!
//! A single record holding both would work and would be one refactor away from
//! `apex capability list` printing credentials.

use serde::{Deserialize, Serialize};

use crate::capability::valid_secret_name;
use crate::paths::{self, Layout};
use crate::provider;
use crate::seal::{self, Sealing};
use crate::value::SecretValue;

/// What is known about a stored credential. **Never the credential.**
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretMeta {
    /// The name a caller refers to it by.
    pub name: String,
    /// Which provider's vocabulary applies to it.
    pub provider: String,
    /// The endpoint it is valid for, when it is not the provider's default —
    /// a GitHub Enterprise host, a test double. Settable only by an
    /// administrator, never by the caller of an operation.
    #[serde(default)]
    pub base_url: Option<String>,
    /// A note from whoever stored it. Human text, no meaning to the service.
    #[serde(default)]
    pub label: Option<String>,
    pub sealing: Sealing,
    pub created_ms: u64,
    /// When it was last replaced in place. `None` until the first rotation.
    #[serde(default)]
    pub rotated_ms: Option<u64>,
}

impl SecretMeta {
    /// The endpoint operations on this credential are sent to.
    pub fn base(&self) -> Option<&str> {
        match &self.base_url {
            Some(b) => Some(b.as_str()),
            None => provider::provider(&self.provider).map(|p| p.default_base),
        }
    }
}

/// Why a store operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    BadName(String),
    UnknownProvider(String),
    BadBaseUrl(String),
    NoSuchSecret(String),
    /// A name that is already taken. Replacing a credential is `rotate`, which
    /// says so in the audit log; `store` refusing means an accidental overwrite
    /// cannot silently break an existing grant.
    Exists(String),
    Seal(seal::SealError),
    Io(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::BadName(n) => write!(
                f,
                "'{}' is not a credential name; use letters, digits, _ - and .",
                n.escape_debug()
            ),
            StoreError::UnknownProvider(p) => write!(
                f,
                "'{}' is not a provider this build knows; known providers: {}",
                p.escape_debug(),
                provider::PROVIDERS
                    .iter()
                    .map(|p| p.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            StoreError::BadBaseUrl(u) => write!(
                f,
                "'{}' is not an absolute http or https origin",
                u.escape_debug()
            ),
            StoreError::NoSuchSecret(n) => {
                write!(f, "no credential called '{}' is stored", n.escape_debug())
            }
            StoreError::Exists(n) => write!(
                f,
                "'{n}' is already stored; replace it with `apex capability rotate {n}`, \
                 which keeps its grants and records the change"
            ),
            StoreError::Seal(e) => write!(f, "{e}"),
            StoreError::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// What an administrator supplies when putting a credential in.
#[derive(Debug, Clone)]
pub struct NewSecret {
    pub name: String,
    pub provider: String,
    pub base_url: Option<String>,
    pub label: Option<String>,
    pub sealing: Sealing,
}

/// Put a credential in. Refuses to overwrite; see [`rotate`].
pub fn store(
    layout: &Layout,
    uid: u32,
    new: &NewSecret,
    value: &SecretValue,
) -> Result<SecretMeta, StoreError> {
    validate(new)?;
    if layout.meta_file(uid, &new.name).exists() {
        return Err(StoreError::Exists(new.name.clone()));
    }
    write(
        layout,
        uid,
        SecretMeta {
            name: new.name.clone(),
            provider: new.provider.clone(),
            base_url: new.base_url.clone(),
            label: new.label.clone(),
            sealing: new.sealing,
            created_ms: crate::now_ms(),
            rotated_ms: None,
        },
        value,
    )
}

/// Replace the value of a credential that already exists, keeping its grants.
///
/// A separate verb from [`store`] because the two have different consequences:
/// storing creates something nothing is granted on yet, rotating changes what
/// every existing grant now uses. The audit log distinguishes them.
pub fn rotate(
    layout: &Layout,
    uid: u32,
    name: &str,
    value: &SecretValue,
) -> Result<SecretMeta, StoreError> {
    let mut meta = meta(layout, uid, name)?;
    meta.rotated_ms = Some(crate::now_ms());
    write(layout, uid, meta, value)
}

fn validate(new: &NewSecret) -> Result<(), StoreError> {
    if !valid_secret_name(&new.name) {
        return Err(StoreError::BadName(new.name.clone()));
    }
    if provider::provider(&new.provider).is_none() {
        return Err(StoreError::UnknownProvider(new.provider.clone()));
    }
    if let Some(base) = &new.base_url {
        if !provider::valid_base_url(base) {
            return Err(StoreError::BadBaseUrl(base.clone()));
        }
    }
    if let Some(label) = &new.label {
        if label.len() > 200 || label.chars().any(|c| c.is_control()) {
            return Err(StoreError::BadName(label.clone()));
        }
    }
    Ok(())
}

fn write(
    layout: &Layout,
    uid: u32,
    meta: SecretMeta,
    value: &SecretValue,
) -> Result<SecretMeta, StoreError> {
    let blob = seal::seal(meta.sealing, value).map_err(StoreError::Seal)?;
    // The blob first: a metadata record pointing at a blob that was never
    // written would make every later use fail with "no such credential",
    // whereas a blob with no metadata is invisible and harmless.
    paths::write_private(&layout.blob_file(uid, &meta.name), &blob, 0o600)
        .map_err(|e| StoreError::Io(format!("writing the sealed credential: {e}")))?;
    let json = serde_json::to_vec_pretty(&meta)
        .map_err(|e| StoreError::Io(format!("serialising the record: {e}")))?;
    paths::write_private(&layout.meta_file(uid, &meta.name), &json, 0o600)
        .map_err(|e| StoreError::Io(format!("writing the record: {e}")))?;
    Ok(meta)
}

/// One credential's metadata.
pub fn meta(layout: &Layout, uid: u32, name: &str) -> Result<SecretMeta, StoreError> {
    if !valid_secret_name(name) {
        return Err(StoreError::BadName(name.to_string()));
    }
    let text = std::fs::read_to_string(layout.meta_file(uid, name))
        .map_err(|_| StoreError::NoSuchSecret(name.to_string()))?;
    serde_json::from_str(&text).map_err(|e| StoreError::Io(format!("reading the record: {e}")))
}

/// Every credential this owner has, metadata only, sorted by name.
pub fn list(layout: &Layout, uid: u32) -> Vec<SecretMeta> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(layout.secrets_dir(uid)) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(meta) = serde_json::from_str::<SecretMeta>(&text) {
                out.push(meta);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Remove a credential and its value.
pub fn remove(layout: &Layout, uid: u32, name: &str) -> Result<(), StoreError> {
    // Read the metadata first, so a bad name or a missing credential is
    // reported before anything is deleted.
    meta(layout, uid, name)?;
    // The blob first again, for the same reason as writing: leftover metadata
    // fails closed, a leftover blob does not.
    let _ = std::fs::remove_file(layout.blob_file(uid, name));
    std::fs::remove_file(layout.meta_file(uid, name))
        .map_err(|e| StoreError::Io(format!("removing the record: {e}")))
}

/// Read a credential back, for the one caller that performs an operation.
///
/// The only function here that produces a [`SecretValue`]. It is deliberately
/// not called `get`: a name that reads like an accessor invites use, and there
/// is exactly one place this belongs.
pub fn open(layout: &Layout, uid: u32, name: &str) -> Result<(SecretMeta, SecretValue), StoreError> {
    let meta = meta(layout, uid, name)?;
    let blob = std::fs::read(layout.blob_file(uid, name))
        .map_err(|_| StoreError::NoSuchSecret(name.to_string()))?;
    let value = seal::unseal(meta.sealing, &blob).map_err(StoreError::Seal)?;
    Ok((meta, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(Layout, std::path::PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.1).ok();
        }
    }

    fn temp(tag: &str) -> Temp {
        let base = std::env::temp_dir().join(format!(
            "apex-secret-store-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&base).ok();
        let layout = Layout::new(base.join("state"), base.join("run"));
        Temp(layout, base)
    }

    fn github(name: &str) -> NewSecret {
        NewSecret {
            name: name.to_string(),
            provider: "github".to_string(),
            base_url: None,
            label: None,
            sealing: Sealing::Plain,
        }
    }

    fn value(v: &str) -> SecretValue {
        SecretValue::new(v).expect("a test value")
    }

    #[test]
    fn a_credential_goes_in_and_only_its_metadata_comes_out() {
        let t = temp("roundtrip");
        let l = &t.0;
        let meta = store(l, 1000, &github("gh"), &value("not-a-real-token-store")).unwrap();
        assert_eq!(meta.provider, "github");
        assert_eq!(meta.base(), Some("https://api.github.com"));

        // The metadata type is what `list` and the API return, and it must not
        // be able to carry the value even by accident.
        let json = serde_json::to_string(&list(l, 1000)).unwrap();
        assert!(!json.contains("not-a-real-token-store"), "{json}");

        // The value is reachable only through `open`.
        let (_, v) = open(l, 1000, "gh").unwrap();
        assert_eq!(v.expose(), "not-a-real-token-store");
    }

    #[test]
    fn the_files_are_not_readable_by_anyone_but_the_service() {
        use std::os::unix::fs::PermissionsExt;

        let t = temp("modes");
        let l = &t.0;
        store(l, 1000, &github("gh"), &value("not-a-real-token-modes")).unwrap();
        for p in [l.meta_file(1000, "gh"), l.blob_file(1000, "gh")] {
            let mode = std::fs::metadata(&p).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{} was {:o}", p.display(), mode & 0o777);
        }
        for p in [l.secrets_dir(1000), l.owner_dir(1000)] {
            let mode = std::fs::metadata(&p).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{} was {:o}", p.display(), mode & 0o777);
        }
    }

    #[test]
    fn one_owners_credentials_are_invisible_to_another() {
        // The uid is a directory component the caller never supplies, so this
        // holds without any check in the lookup path.
        let t = temp("owners");
        let l = &t.0;
        store(l, 1000, &github("gh"), &value("not-a-real-token-a")).unwrap();
        store(l, 1001, &github("gh"), &value("not-a-real-token-b")).unwrap();
        assert_eq!(open(l, 1000, "gh").unwrap().1.expose(), "not-a-real-token-a");
        assert_eq!(open(l, 1001, "gh").unwrap().1.expose(), "not-a-real-token-b");
        assert!(list(l, 1002).is_empty());
        assert!(matches!(
            open(l, 1002, "gh"),
            Err(StoreError::NoSuchSecret(_))
        ));
    }

    #[test]
    fn storing_over_an_existing_credential_is_refused_and_rotating_is_not() {
        let t = temp("rotate");
        let l = &t.0;
        store(l, 1000, &github("gh"), &value("not-a-real-token-first")).unwrap();
        assert!(matches!(
            store(l, 1000, &github("gh"), &value("not-a-real-token-second")),
            Err(StoreError::Exists(_))
        ));
        let meta = rotate(l, 1000, "gh", &value("not-a-real-token-second")).unwrap();
        assert!(meta.rotated_ms.is_some());
        assert_eq!(meta.created_ms, store_created(l, 1000, "gh"));
        assert_eq!(open(l, 1000, "gh").unwrap().1.expose(), "not-a-real-token-second");
        assert!(matches!(
            rotate(l, 1000, "absent", &value("x")),
            Err(StoreError::NoSuchSecret(_))
        ));
    }

    fn store_created(l: &Layout, uid: u32, name: &str) -> u64 {
        meta(l, uid, name).unwrap().created_ms
    }

    #[test]
    fn a_bad_name_or_provider_is_refused_before_anything_is_written() {
        let t = temp("validate");
        let l = &t.0;
        let mut bad = github("../escape");
        assert!(matches!(
            store(l, 1000, &bad, &value("x")),
            Err(StoreError::BadName(_))
        ));
        bad.name = "gh".into();
        bad.provider = "gitlab".into();
        assert!(matches!(
            store(l, 1000, &bad, &value("x")),
            Err(StoreError::UnknownProvider(_))
        ));
        bad.provider = "github".into();
        bad.base_url = Some("https://user@evil.example".into());
        assert!(matches!(
            store(l, 1000, &bad, &value("x")),
            Err(StoreError::BadBaseUrl(_))
        ));
        // Nothing reached the disk.
        assert!(list(l, 1000).is_empty());
        assert!(!l.secrets_dir(1000).join("..").join("escape.json").exists());
    }

    #[test]
    fn a_stored_base_url_overrides_the_providers_default() {
        let t = temp("base");
        let l = &t.0;
        let mut n = github("gh");
        n.base_url = Some("http://127.0.0.1:9".into());
        n.label = Some("a test double".into());
        let meta = store(l, 1000, &n, &value("not-a-real-token-base")).unwrap();
        assert_eq!(meta.base(), Some("http://127.0.0.1:9"));
        assert_eq!(meta.label.as_deref(), Some("a test double"));
    }

    #[test]
    fn removing_takes_the_value_with_it() {
        let t = temp("remove");
        let l = &t.0;
        store(l, 1000, &github("gh"), &value("not-a-real-token-remove")).unwrap();
        remove(l, 1000, "gh").unwrap();
        assert!(!l.blob_file(1000, "gh").exists(), "the value outlived the record");
        assert!(matches!(
            open(l, 1000, "gh"),
            Err(StoreError::NoSuchSecret(_))
        ));
        assert!(matches!(
            remove(l, 1000, "gh"),
            Err(StoreError::NoSuchSecret(_))
        ));
    }

    #[test]
    fn a_record_whose_blob_is_missing_fails_closed() {
        // What a half-finished delete or a restored backup looks like. It must
        // refuse, not hand a caller an empty credential.
        let t = temp("halfgone");
        let l = &t.0;
        store(l, 1000, &github("gh"), &value("not-a-real-token-half")).unwrap();
        std::fs::remove_file(l.blob_file(1000, "gh")).unwrap();
        assert!(matches!(
            open(l, 1000, "gh"),
            Err(StoreError::NoSuchSecret(_))
        ));
        // But it is still listed, so the owner can see what to fix.
        assert_eq!(list(l, 1000).len(), 1);
    }

    #[test]
    fn a_corrupt_record_is_skipped_by_listing_rather_than_hiding_the_rest() {
        let t = temp("corrupt");
        let l = &t.0;
        store(l, 1000, &github("good"), &value("not-a-real-token-good")).unwrap();
        paths::write_private(&l.meta_file(1000, "broken"), b"{ not json", 0o600).unwrap();
        let all = list(l, 1000);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "good");
    }
}
