//! Releases: whether a newer one is out, and proof that one is really Kyte's.
//!
//! A release carries `SHA256SUMS` and `SHA256SUMS.sig`, an ed25519 signature
//! over the sums made by the release workflow with a key only it holds. The
//! public half is built into Kyte, so a mirror or a tampered release can't pass
//! for a real one. A checksum from the same place as the file would only catch
//! a bad download.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};

/// Where releases come from.
pub const REPO: &str = "zypher-systems/kyte";

/// This build's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Set by the release workflow on the binaries it publishes. A build from
/// source doesn't have it, wherever that build is copied to.
pub const RELEASE_BUILD: bool = option_env!("KYTE_RELEASE_BUILD").is_some();

/// How often the launch check asks.
const CHECK_EVERY_SECS: u64 = 24 * 60 * 60;

/// The public half of the release signing key.
const RELEASE_KEY_PEM: &str = include_str!("../../release/kyte-release.pub.pem");

/// What `SHA256SUMS` and its signature may weigh.
const MAX_SUMS: u64 = 64 << 10;
const MAX_SIGNATURE: u64 = 4 << 10;
/// What GitHub's answer may weigh.
const MAX_ANSWER: u64 = 1 << 20;
/// What a release file may weigh.
const MAX_FILE: u64 = 200 << 20;

/// MAJOR.MINOR.PATCH. Pre-releases and anything else don't parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// `0.6.0` or `v0.6.0`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('v').unwrap_or(s);
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        let n = |p: &str| -> Option<u32> {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            p.parse().ok()
        };
        Some(Self(n(parts[0])?, n(parts[1])?, n(parts[2])?))
    }

    /// This build.
    pub fn current() -> Self {
        Self::parse(CURRENT).unwrap_or(Self(0, 0, 0))
    }

    /// The release tag, `v0.6.0`.
    pub fn tag(self) -> String {
        format!("v{self}")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// A published release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// Its page, with the notes. Empty when the answer gave none.
    pub url: String,
}

/// Reads GitHub's answer for the latest release. Drafts and pre-releases never
/// count.
pub fn parse_release(json: &[u8]) -> Result<Release, String> {
    let answer: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| format!("couldn't read GitHub's answer: {e}"))?;
    let tag = answer["tag_name"].as_str().unwrap_or_default();
    if answer["draft"] == true || answer["prerelease"] == true {
        return Err(format!("{tag:?} isn't a published release"));
    }
    let version = Version::parse(tag).ok_or_else(|| format!("{tag:?} isn't a version"))?;
    let url = answer["html_url"].as_str().unwrap_or_default();
    // It gets printed, and it came from the network.
    let url = if url.chars().any(char::is_control) {
        ""
    } else {
        url
    };
    Ok(Release {
        version,
        url: url.to_string(),
    })
}

/// The raw key in an ed25519 `PUBLIC KEY` PEM (what `openssl pkey -pubout`
/// writes): SubjectPublicKeyInfo is a fixed 12-byte prefix, then the key.
pub fn key_from_pem(pem: &str) -> Option<[u8; 32]> {
    const PREFIX: [u8; 12] = [
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    let body: String = pem
        .lines()
        .skip_while(|l| !l.starts_with("-----BEGIN PUBLIC KEY-----"))
        .skip(1)
        .take_while(|l| !l.starts_with("-----END"))
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body.trim())
        .ok()?;
    der.strip_prefix(&PREFIX[..])?.try_into().ok()
}

/// The key a release must be signed with: the release key, or
/// `KYTE_UPDATE_PUBKEY` at build time (the raw key in base64, for a fork).
pub fn built_in_key() -> Option<[u8; 32]> {
    match option_env!("KYTE_UPDATE_PUBKEY") {
        Some(b64) => base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .ok()?
            .try_into()
            .ok(),
        None => key_from_pem(RELEASE_KEY_PEM),
    }
}

/// `sums` (a release's `SHA256SUMS`) carries `sig` (`SHA256SUMS.sig`, the
/// signature in base64) made with `key`. Anything else is refused: a release
/// signed with another key, or checksums changed after signing.
pub fn check_signature(key: Option<[u8; 32]>, sums: &[u8], sig: &[u8]) -> Result<(), String> {
    let key = key
        .and_then(|k| VerifyingKey::from_bytes(&k).ok())
        .ok_or("this build has no release key to check releases with; nothing was installed")?;
    let refused = || {
        "the release's signature doesn't match Kyte's release key; nothing was installed"
            .to_string()
    };
    let sig = std::str::from_utf8(sig).map_err(|_| refused())?;
    let sig = base64::engine::general_purpose::STANDARD
        .decode(sig.trim())
        .map_err(|_| refused())?;
    let sig = Signature::from_slice(&sig).map_err(|_| refused())?;
    key.verify_strict(sums, &sig).map_err(|_| refused())
}

/// The hash `SHA256SUMS` lists for `name` (`<hex>  name` or `<hex> *name`).
pub fn listed_sum(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (hash, rest) = l.split_once(char::is_whitespace)?;
        let file = rest.trim_start().trim_start_matches('*').trim_end();
        (file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// The SHA-256 of `bytes`, in hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Checks `bytes` against `SHA256SUMS`. A file that isn't listed is refused,
/// never waved through.
pub fn check_sum(name: &str, bytes: &[u8], sums: &str) -> Result<(), String> {
    let want = listed_sum(sums, name).ok_or_else(|| format!("{name} isn't in SHA256SUMS"))?;
    if sha256_hex(bytes) == want {
        Ok(())
    } else {
        Err(format!(
            "{name} doesn't match its checksum; nothing was installed"
        ))
    }
}

/// This machine's release target.
pub fn target() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("x86_64-unknown-linux-musl"),
        "aarch64" => Some("aarch64-unknown-linux-musl"),
        _ => None,
    }
}

/// The release tarball for `target`.
pub fn tarball_name(target: &str) -> String {
    format!("kyte-{target}.tar.gz")
}

/// The release RPM for this machine.
pub fn rpm_name(v: Version) -> String {
    format!("kyte-{v}-1.{}.rpm", std::env::consts::ARCH)
}

/// The release .deb for this machine (Debian names the architectures its own
/// way).
pub fn deb_name(v: Version) -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("kyte_{v}-1_{arch}.deb")
}

/// Where releases come from, and the key they must be signed with.
#[derive(Debug, Clone)]
pub struct Releases {
    /// GitHub's latest-release answer.
    pub latest: String,
    /// One release's files, instead of GitHub's (a mirror).
    pub mirror: Option<String>,
    /// The signing key.
    pub key: Option<[u8; 32]>,
}

impl Default for Releases {
    /// GitHub, or `KYTE_RELEASES_API` and `KYTE_DOWNLOAD_URL`, the mirror
    /// `install.sh` also takes. A mirror's release must carry the same
    /// signature, so it can't change what's installed.
    fn default() -> Self {
        let set = |name| std::env::var(name).ok().filter(|s| !s.trim().is_empty());
        Self {
            latest: set("KYTE_RELEASES_API")
                .unwrap_or_else(|| format!("https://api.github.com/repos/{REPO}/releases/latest")),
            mirror: set("KYTE_DOWNLOAD_URL").map(|u| u.trim_end_matches('/').to_string()),
            key: built_in_key(),
        }
    }
}

impl Releases {
    /// Where `v`'s files are.
    pub fn base(&self, v: Version) -> String {
        match &self.mirror {
            Some(m) => m.clone(),
            None => format!("https://github.com/{REPO}/releases/download/{}", v.tag()),
        }
    }

    /// The newest published release.
    pub fn latest(&self) -> Result<Release, String> {
        let mut answer = agent(Duration::from_secs(15))
            .get(&self.latest)
            .header("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| match e {
                ureq::Error::StatusCode(404) => "no published release yet".to_string(),
                ureq::Error::StatusCode(code) => format!("GitHub answered {code}"),
                e => format!("couldn't reach GitHub: {e}"),
            })?;
        let body = answer
            .body_mut()
            .with_config()
            .limit(MAX_ANSWER)
            .read_to_vec()
            .map_err(|e| format!("couldn't read GitHub's answer: {e}"))?;
        parse_release(&body)
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(format!("kyte/{CURRENT}"))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(timeout))
        .build()
        .new_agent()
}

/// `url`'s body, refusing more than `max` bytes.
fn get(url: &str, max: u64) -> Result<Vec<u8>, ureq::Error> {
    agent(Duration::from_secs(600))
        .get(url)
        .call()?
        .body_mut()
        .with_config()
        .limit(max)
        .read_to_vec()
}

/// A release whose `SHA256SUMS` carries Kyte's signature. Nothing else is
/// downloaded until that's so.
pub struct Signed {
    base: String,
    sums: String,
    dir: PathBuf,
}

impl Signed {
    /// Downloads `to`'s checksums and signature, and refuses them unless Kyte's
    /// release key signed them. Files land in `dir`.
    pub fn fetch(releases: &Releases, to: Version, dir: &Path) -> Result<Self, String> {
        let base = releases.base(to);
        let sums = get(&format!("{base}/SHA256SUMS"), MAX_SUMS)
            .map_err(|e| format!("couldn't download {to}'s SHA256SUMS: {e}"))?;
        let sig = get(&format!("{base}/SHA256SUMS.sig"), MAX_SIGNATURE).map_err(|e| match e {
            ureq::Error::StatusCode(404) => format!("{to} isn't signed; nothing was installed"),
            e => format!("couldn't download {to}'s signature: {e}"),
        })?;
        check_signature(releases.key, &sums, &sig)?;
        println!("SHA256SUMS is signed with Kyte's release key");
        Ok(Self {
            base,
            sums: String::from_utf8_lossy(&sums).into_owned(),
            dir: dir.to_path_buf(),
        })
    }

    /// Downloads `name` into the folder, and refuses it unless it matches.
    pub fn file(&self, name: &str) -> Result<PathBuf, String> {
        // Refused before it costs a download.
        listed_sum(&self.sums, name).ok_or_else(|| format!("{name} isn't in SHA256SUMS"))?;
        println!("downloading {name}");
        let bytes = get(&format!("{}/{name}", self.base), MAX_FILE)
            .map_err(|e| format!("couldn't download {name}: {e}"))?;
        check_sum(name, &bytes, &self.sums)?;
        println!("{name} matches its checksum");
        let path = self.dir.join(name);
        fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }
}

/// What a running Kyte should say about updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    /// A newer release is out.
    Available(Version),
    /// A newer version is already installed; this process predates it.
    Restart(Version),
}

impl Badge {
    /// The line for the status bar.
    pub fn note(self) -> String {
        match self {
            Badge::Available(v) => format!("Kyte {v} is out. Run kyte --update to install it"),
            Badge::Restart(v) => format!("Kyte {v} is installed. Restart Kyte to use it"),
        }
    }
}

/// What's remembered between runs, in `update/state.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// When it last got an answer, in seconds since the epoch.
    pub checked: Option<u64>,
    /// The newest release it found.
    pub latest: Option<String>,
    /// The version `kyte --update` put on disk.
    pub installed: Option<String>,
    /// The version `kyte --update` replaced, for `--update-rollback`.
    pub previous: Option<String>,
}

impl State {
    fn file(dir: &Path) -> PathBuf {
        dir.join("state.json")
    }

    /// Reads it. A missing or damaged file is an empty state: this only ever
    /// costs one more check.
    pub fn load(dir: &Path) -> Self {
        let Ok(text) = fs::read(Self::file(dir)) else {
            return Self::default();
        };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&text) else {
            return Self::default();
        };
        let text = |key: &str| v[key].as_str().map(str::to_string);
        Self {
            checked: v["checked"].as_u64(),
            latest: text("latest"),
            installed: text("installed"),
            previous: text("previous"),
        }
    }

    fn save(&self, dir: &Path) -> io::Result<()> {
        let json = serde_json::json!({
            "checked": self.checked,
            "latest": self.latest,
            "installed": self.installed,
            "previous": self.previous,
        });
        let tmp = dir.join(format!(".state.{}", std::process::id()));
        let written = (|| {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(json.to_string().as_bytes())?;
            f.sync_all()?;
            fs::rename(&tmp, Self::file(dir))
        })();
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        written
    }

    /// Reads, changes and writes it under a lock, so a Kyte starting up and
    /// `kyte --update` never drop each other's fields. The rollback target
    /// above all.
    pub fn modify(dir: &Path, change: impl FnOnce(&mut Self)) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let lock = fs::File::create(dir.join(".lock"))?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)?;
        let mut state = Self::load(dir);
        change(&mut state);
        state.save(dir)?;
        Ok(state)
    }

    /// Whether the launch check should ask. Never asked is due, whatever the
    /// clock says.
    pub fn due(&self, now: u64) -> bool {
        self.checked
            .is_none_or(|last| now.saturating_sub(last) >= CHECK_EVERY_SECS)
    }

    /// What a process of version `running` should say.
    pub fn badge(&self, running: Version) -> Option<Badge> {
        let latest = self.latest.as_deref().and_then(Version::parse);
        let installed = self.installed.as_deref().and_then(Version::parse);
        // Already on disk, and nothing newer out: only a restart is missing.
        if let Some(i) = installed {
            if i > running && latest.is_none_or(|l| i >= l) {
                return Some(Badge::Restart(i));
            }
        }
        latest.filter(|l| *l > running).map(Badge::Available)
    }
}

/// Seconds since the epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The launch check: at most once a day, and only when it's due. A check that
/// fails says nothing and is tried again next launch, so being offline is
/// never reported.
pub fn on_launch(dir: &Path, releases: &Releases, running: Version, now: u64) -> Option<String> {
    if State::load(dir).due(now) {
        if let Ok(found) = releases.latest() {
            let _ = State::modify(dir, |s| {
                s.checked = Some(now);
                s.latest = Some(found.version.to_string());
            });
        }
    }
    State::load(dir).badge(running).map(Badge::note)
}

/// The palette's check: asks now, and always has an answer.
pub fn on_request(dir: &Path, releases: &Releases, running: Version, now: u64) -> String {
    match releases.latest() {
        Ok(found) => {
            let state = State::modify(dir, |s| {
                s.checked = Some(now);
                s.latest = Some(found.version.to_string());
            })
            .unwrap_or_else(|_| State {
                latest: Some(found.version.to_string()),
                ..State::load(dir)
            });
            state.badge(running).map_or_else(
                || format!("Kyte {running} is the newest release"),
                Badge::note,
            )
        }
        Err(e) => format!("Couldn't check for updates: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn versions_are_three_numbers() {
        assert_eq!(Version::parse("0.6.0"), Some(Version(0, 6, 0)));
        assert_eq!(Version::parse("v1.10.2"), Some(Version(1, 10, 2)));
        for bad in [
            "",
            "1.2",
            "1.2.3.4",
            "0.6.0-rc1",
            "v",
            "a.b.c",
            "1..2",
            "+1.2.3",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        assert_eq!(Version(0, 6, 0).tag(), "v0.6.0");
        assert!(Version::parse(CURRENT).is_some(), "this build's version");
    }

    #[test]
    fn only_published_releases_count() {
        let ok = br#"{"tag_name":"v0.7.0","html_url":"https://example.test/v0.7.0","draft":false,"prerelease":false}"#;
        assert_eq!(
            parse_release(ok),
            Ok(Release {
                version: Version(0, 7, 0),
                url: "https://example.test/v0.7.0".into()
            })
        );
        assert!(parse_release(br#"{"tag_name":"v0.7.0","prerelease":true}"#).is_err());
        assert!(parse_release(br#"{"tag_name":"v0.7.0","draft":true}"#).is_err());
        assert!(parse_release(br#"{"tag_name":"nightly"}"#).is_err());
        assert!(parse_release(b"<html>").is_err());
    }

    #[test]
    fn an_address_that_could_drive_the_terminal_is_dropped() {
        let sly = br#"{"tag_name":"v0.7.0","html_url":"https://x\u001b]0;boo\u0007"}"#;
        assert_eq!(parse_release(sly).unwrap().url, "");
    }

    #[test]
    fn the_committed_key_is_a_key() {
        let raw = key_from_pem(RELEASE_KEY_PEM).expect("release/kyte-release.pub.pem");
        assert!(VerifyingKey::from_bytes(&raw).is_ok());
        assert!(built_in_key().is_some());
        assert_eq!(
            key_from_pem("-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----"),
            None
        );
        assert_eq!(key_from_pem("# not yet\n"), None);
    }

    #[test]
    fn a_release_is_taken_only_on_its_signature() {
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        let ours = SigningKey::from_bytes(&[7u8; 32]);
        let key = Some(ours.verifying_key().to_bytes());
        let sums = b"abc  kyte-x86_64-unknown-linux-musl.tar.gz\n";
        let sig = b64(&ours.sign(sums).to_bytes());
        assert_eq!(check_signature(key, sums, sig.as_bytes()), Ok(()));
        // The same signature over other bytes.
        assert!(check_signature(key, b"abd", sig.as_bytes()).is_err());
        // A good signature from another key, however well formed.
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let forged = b64(&other.sign(sums).to_bytes());
        let e = check_signature(key, sums, forged.as_bytes()).unwrap_err();
        assert!(e.contains("doesn't match"), "{e}");
        assert!(check_signature(key, sums, b"not base64!").is_err());
        assert!(check_signature(key, sums, b64(b"short").as_bytes()).is_err());
        assert!(check_signature(key, sums, &[0xff, 0xfe]).is_err());
        let e = check_signature(None, sums, sig.as_bytes()).unwrap_err();
        assert!(e.contains("no release key"), "{e}");
    }

    #[test]
    fn checksums_must_match_and_be_listed() {
        let name = "kyte-x86_64-unknown-linux-musl.tar.gz";
        let sum = sha256_hex(b"payload");
        let sums = format!("{sum}  {name}\n{}  install.sh\n", "0".repeat(64));
        assert_eq!(check_sum(name, b"payload", &sums), Ok(()));
        assert_eq!(
            check_sum(name, b"payload", &format!("{sum} *{name}\n")),
            Ok(())
        );
        assert!(check_sum(name, b"other", &sums)
            .unwrap_err()
            .contains("checksum"));
        assert!(check_sum(name, b"payload", "")
            .unwrap_err()
            .contains("isn't in"));
        // A name that only ends the same doesn't count.
        assert!(check_sum(name, b"payload", &format!("{sum}  old-{name}\n")).is_err());
        // Nor does a line that isn't a hash.
        assert_eq!(listed_sum(&format!("abc  {name}\n"), name), None);
    }

    #[test]
    fn release_files_are_named_as_the_workflow_names_them() {
        let v = Version(0, 6, 0);
        match std::env::consts::ARCH {
            "x86_64" => {
                assert_eq!(rpm_name(v), "kyte-0.6.0-1.x86_64.rpm");
                assert_eq!(deb_name(v), "kyte_0.6.0-1_amd64.deb");
            }
            "aarch64" => {
                assert_eq!(rpm_name(v), "kyte-0.6.0-1.aarch64.rpm");
                assert_eq!(deb_name(v), "kyte_0.6.0-1_arm64.deb");
            }
            _ => {}
        }
        assert_eq!(
            tarball_name("x86_64-unknown-linux-musl"),
            "kyte-x86_64-unknown-linux-musl.tar.gz"
        );
    }

    #[test]
    fn the_badge_says_update_or_restart() {
        let running = Version(0, 6, 0);
        let mut s = State::default();
        assert_eq!(s.badge(running), None);
        s.latest = Some("0.6.0".into());
        assert_eq!(s.badge(running), None, "same version: nothing");
        s.latest = Some("0.7.0".into());
        assert_eq!(s.badge(running), Some(Badge::Available(Version(0, 7, 0))));
        // Updated on disk while this one ran: restart.
        s.installed = Some("0.7.0".into());
        assert_eq!(s.badge(running), Some(Badge::Restart(Version(0, 7, 0))));
        // Installed is newer than this, but it's still not the newest.
        s.latest = Some("0.8.0".into());
        assert_eq!(s.badge(running), Some(Badge::Available(Version(0, 8, 0))));
        // What's on disk is older than this copy.
        s.installed = Some("0.5.0".into());
        s.latest = Some("0.6.0".into());
        assert_eq!(s.badge(running), None);
    }

    #[test]
    fn the_launch_check_is_daily() {
        let mut s = State::default();
        assert!(s.due(1_000_000));
        s.checked = Some(1_000_000);
        assert!(!s.due(1_000_000 + 3600));
        assert!(s.due(1_000_000 + CHECK_EVERY_SECS));
        // A clock that went back doesn't ask on every launch.
        assert!(!s.due(10));
    }

    #[test]
    fn the_state_keeps_what_each_writer_put_there() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("update");
        assert_eq!(State::load(&dir), State::default());
        State::modify(&dir, |s| s.previous = Some("0.6.0".into())).unwrap();
        let after = State::modify(&dir, |s| {
            s.checked = Some(5);
            s.latest = Some("0.7.0".into());
        })
        .unwrap();
        assert_eq!(after.previous.as_deref(), Some("0.6.0"));
        assert_eq!(State::load(&dir), after);
    }

    #[test]
    fn a_damaged_state_is_an_empty_one() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json"), "{ not json").unwrap();
        assert_eq!(State::load(d.path()), State::default());
        assert!(State::modify(d.path(), |s| s.checked = Some(1)).is_ok());
    }

    /// Nothing listens on this port, so the answer is a failed connection.
    fn offline() -> Releases {
        Releases {
            latest: "http://127.0.0.1:1/latest".into(),
            mirror: None,
            key: None,
        }
    }

    #[test]
    fn offline_says_nothing_at_launch_and_tries_again() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(on_launch(d.path(), &offline(), Version(0, 6, 0), 100), None);
        assert!(State::load(d.path()).due(101), "tried again next launch");
    }

    #[test]
    fn a_remembered_answer_needs_no_network() {
        let d = tempfile::tempdir().unwrap();
        State::modify(d.path(), |s| {
            s.checked = Some(100);
            s.latest = Some("0.7.0".into());
        })
        .unwrap();
        let said = on_launch(d.path(), &offline(), Version(0, 6, 0), 200).unwrap();
        assert!(
            said.contains("0.7.0") && said.contains("kyte --update"),
            "{said}"
        );
    }

    #[test]
    fn asking_offline_gets_an_answer() {
        let d = tempfile::tempdir().unwrap();
        let said = on_request(d.path(), &offline(), Version(0, 6, 0), 100);
        assert!(said.starts_with("Couldn't check for updates"), "{said}");
    }
}
