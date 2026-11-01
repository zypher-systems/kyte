//! The updater from the download to the file on disk: a release served from a
//! local port, signed with a test key, and put in place by the real install
//! script. Every way a release can be wrong must leave the installed copy alone.

#![cfg(feature = "update")]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use kyte::update::apply::{execute, version_of, Kind};
use kyte::update::release::{self, sha256_hex, Releases, Version};

const OLD: &str = "#!/bin/sh\necho 'kyte 0.1.0'\n";

/// The test skips where these can't run, so it doesn't lean on the machine.
fn have_tools() -> bool {
    ["tar", "sh"].iter().all(|t| {
        Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {t}"))
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn target() -> &'static str {
    release::target().expect("a supported test machine")
}

fn tarball_name() -> String {
    release::tarball_name(target())
}

/// A release's tarball, whose `kyte` is `script`.
fn tarball(d: &Path, script: &str) -> Vec<u8> {
    let name = format!("kyte-{}", target());
    let stage = d.join(&name);
    fs::create_dir_all(&stage).unwrap();
    let bin = stage.join("kyte");
    fs::write(&bin, script).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    let out = d.join("t.tar.gz");
    let ok = Command::new("tar")
        .arg("-C")
        .arg(d)
        .arg("-czf")
        .arg(&out)
        .arg(&name)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    fs::read(out).unwrap()
}

/// A release's files: name and body.
type Files = Vec<(String, Vec<u8>)>;

/// A complete signed release of `script`, with the real install script.
fn signed_release(script: &str) -> Files {
    let d = tempfile::tempdir().unwrap();
    let tgz = tarball(d.path(), script);
    let installer = fs::read("install.sh").unwrap();
    let mut files = vec![(tarball_name(), tgz), ("install.sh".to_string(), installer)];
    sign(&mut files);
    files
}

/// Writes the checksums and signature for `files`.
fn sign(files: &mut Files) {
    files.retain(|(n, _)| !n.starts_with("SHA256SUMS"));
    let sums: String = files
        .iter()
        .map(|(n, b)| format!("{}  {n}\n", sha256_hex(b)))
        .collect();
    let sig = base64::engine::general_purpose::STANDARD
        .encode(signing_key().sign(sums.as_bytes()).to_bytes());
    files.push(("SHA256SUMS".to_string(), sums.into_bytes()));
    files.push(("SHA256SUMS.sig".to_string(), sig.into_bytes()));
}

/// Serves `files` from a local port, and returns its address.
fn serve(files: Files) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                    break;
                }
            }
            let path = line.split_whitespace().nth(1).unwrap_or("");
            let name = path.trim_start_matches('/');
            let mut stream = stream;
            match files.iter().find(|(n, _)| n == name) {
                Some((_, body)) => {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(body);
                }
                None => {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                    );
                }
            }
        }
    });
    url
}

fn releases(url: &str) -> Releases {
    Releases {
        latest: format!("{url}/latest"),
        mirror: Some(url.to_string()),
        key: Some(signing_key().verifying_key().to_bytes()),
    }
}

/// An installed 0.1.0 in a `bin` folder, and a private download folder.
struct Machine {
    _home: tempfile::TempDir,
    bin: PathBuf,
    work: PathBuf,
}

impl Machine {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("bin");
        let work = home.path().join("work");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(&work).unwrap();
        let bin = dir.join("kyte");
        fs::write(&bin, OLD).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            _home: home,
            bin,
            work,
        }
    }

    fn update(&self, releases: &Releases, to: Version) -> Result<String, String> {
        let kind = Kind::Installer {
            into: self.bin.parent().unwrap().to_path_buf(),
            sudo: false,
        };
        execute(&kind, to, releases, &self.work)
    }

    /// The copy is as it was, and nothing is left beside it.
    fn assert_untouched(&self, why: &str) {
        assert_eq!(fs::read_to_string(&self.bin).unwrap(), OLD, "{why}");
        let dir = self.bin.parent().unwrap();
        assert_eq!(fs::read_dir(dir).unwrap().count(), 1, "{why}: left behind");
    }
}

const TO: Version = Version(9, 9, 9);
const NEW: &str = "#!/bin/sh\necho 'kyte 9.9.9'\n";

#[test]
fn a_signed_release_replaces_the_copy() {
    if !have_tools() {
        return;
    }
    let machine = Machine::new();
    let url = serve(signed_release(NEW));
    machine.update(&releases(&url), TO).unwrap();
    assert_eq!(version_of(&machine.bin), Some(TO));
    let mode = fs::metadata(&machine.bin).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
    let left = fs::read_dir(machine.bin.parent().unwrap()).unwrap().count();
    assert_eq!(left, 1, "no staging file left behind");
}

/// The swap is a rename, so a Kyte that's open keeps running the old file.
#[test]
fn an_open_copy_keeps_its_file() {
    if !have_tools() {
        return;
    }
    let machine = Machine::new();
    let open = fs::File::open(&machine.bin).unwrap();
    let url = serve(signed_release(NEW));
    machine.update(&releases(&url), TO).unwrap();
    let mut was = String::new();
    std::io::Read::read_to_string(&mut &open, &mut was).unwrap();
    assert_eq!(was, OLD);
}

#[test]
fn a_mirror_cant_change_what_is_installed() {
    if !have_tools() {
        return;
    }
    // The release as a mirror would change it, and what must stop it.
    type Tamper = (&'static str, Box<dyn Fn(&mut Files)>);
    let body = |f: &mut Files, name: &str, new: Vec<u8>| {
        f.iter_mut().find(|(n, _)| n == name).unwrap().1 = new;
    };
    let cases: Vec<Tamper> = vec![
        (
            "signature",
            Box::new(|f| {
                // Checksums rewritten to fit a changed file: only the key stops it.
                let name = tarball_name();
                let at = f.iter().position(|(n, _)| *n == name).unwrap();
                f[at].1.push(0);
                let sums = format!("{}  {name}\n", sha256_hex(&f[at].1));
                let at = f.iter().position(|(n, _)| n == "SHA256SUMS").unwrap();
                f[at].1 = sums.into_bytes();
            }),
        ),
        (
            "signature",
            Box::new(move |f| {
                let other = SigningKey::from_bytes(&[9u8; 32]);
                let sums = f.iter().find(|(n, _)| n == "SHA256SUMS").unwrap().1.clone();
                let forged =
                    base64::engine::general_purpose::STANDARD.encode(other.sign(&sums).to_bytes());
                body(f, "SHA256SUMS.sig", forged.into_bytes());
            }),
        ),
        (
            "isn't signed",
            Box::new(|f| f.retain(|(n, _)| n != "SHA256SUMS.sig")),
        ),
        (
            "checksum",
            Box::new(move |f| {
                let name = tarball_name();
                let at = f.iter().position(|(n, _)| *n == name).unwrap();
                f[at].1.push(0);
            }),
        ),
        (
            "checksum",
            Box::new(|f| {
                // The script that would run is checked as closely as the binary.
                let at = f.iter().position(|(n, _)| n == "install.sh").unwrap();
                f[at].1.extend_from_slice(b"\nrm -rf \"$HOME\"\n");
            }),
        ),
        (
            "isn't in SHA256SUMS",
            Box::new(|f| {
                f.retain(|(n, _)| n != "install.sh");
                sign(f);
                f.push(("install.sh".to_string(), b"#!/bin/sh\n".to_vec()));
            }),
        ),
    ];
    for (why, tamper) in cases {
        let machine = Machine::new();
        let mut files = signed_release(NEW);
        tamper(&mut files);
        let e = machine.update(&releases(&serve(files)), TO).expect_err(why);
        assert!(e.contains(why), "{why}: {e}");
        machine.assert_untouched(why);
    }
}

/// An old release is really signed, so only its version can give it away.
#[test]
fn an_old_signed_release_cant_pass_for_a_new_one() {
    if !have_tools() {
        return;
    }
    let machine = Machine::new();
    let url = serve(signed_release("#!/bin/sh\necho 'kyte 6.6.6'\n"));
    let e = machine.update(&releases(&url), TO).unwrap_err();
    assert!(e.contains("holds Kyte 6.6.6"), "{e}");
    machine.assert_untouched("wrong version");
}

#[test]
fn a_binary_that_doesnt_run_is_not_installed() {
    if !have_tools() {
        return;
    }
    let machine = Machine::new();
    let url = serve(signed_release("#!/bin/sh\nexit 1\n"));
    let e = machine.update(&releases(&url), TO).unwrap_err();
    assert!(e.contains("no working kyte"), "{e}");
    machine.assert_untouched("doesn't run");
}

#[test]
fn a_build_without_a_key_installs_nothing() {
    if !have_tools() {
        return;
    }
    let machine = Machine::new();
    let mut src = releases(&serve(signed_release(NEW)));
    src.key = None;
    let e = machine.update(&src, TO).unwrap_err();
    assert!(e.contains("no release key"), "{e}");
    machine.assert_untouched("no key");
}

#[test]
fn a_missing_release_is_said_plainly() {
    if !have_tools() {
        return;
    }
    let machine = Machine::new();
    let e = machine
        .update(&releases(&serve(Vec::new())), TO)
        .unwrap_err();
    assert!(e.contains("couldn't download"), "{e}");
    machine.assert_untouched("nothing there");
}

#[test]
fn the_latest_release_is_read_from_the_answer() {
    let answer = br#"{"tag_name":"v9.9.9","html_url":"https://example.test/v9.9.9"}"#.to_vec();
    let url = serve(vec![("latest".to_string(), answer)]);
    let found = releases(&url).latest().unwrap();
    assert_eq!(found.version, TO);
    assert_eq!(found.url, "https://example.test/v9.9.9");
    let e = releases(&serve(Vec::new())).latest().unwrap_err();
    assert_eq!(e, "no published release yet");
}

#[test]
fn the_launch_check_remembers_its_answer() {
    let answer = br#"{"tag_name":"v9.9.9"}"#.to_vec();
    let url = serve(vec![("latest".to_string(), answer)]);
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("update");
    let said = release::on_launch(&dir, &releases(&url), Version(0, 6, 0), 100).unwrap();
    assert!(
        said.contains("9.9.9") && said.contains("kyte --update"),
        "{said}"
    );
    // Still known the next day's first launch, with the network gone.
    let gone = releases("http://127.0.0.1:1");
    let again = release::on_launch(&dir, &gone, Version(0, 6, 0), 200).unwrap();
    assert_eq!(said, again);
    assert_eq!(
        release::on_launch(&dir, &gone, TO, 200),
        None,
        "nothing to say to the newest version"
    );
}

/// The real binary. A test build isn't a release, wherever cargo puts it.
fn kyte(state: &Path, url: &str, args: &[&str], answer: &str) -> (bool, String) {
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_kyte"))
        .args(args)
        .env("KYTE_STATE_DIR", state)
        .env("KYTE_RELEASES_API", format!("{url}/latest"))
        .env("KYTE_DOWNLOAD_URL", url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(answer.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let said =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), said)
}

/// It refuses to run as root, so there's nothing to see there.
fn as_root() -> bool {
    rustix::process::geteuid().is_root()
}

fn newer_release() -> String {
    let answer = br#"{"tag_name":"v99.0.0","html_url":"https://example.test/v99"}"#.to_vec();
    serve(vec![("latest".to_string(), answer)])
}

#[test]
fn check_says_a_release_is_out_and_installs_nothing() {
    if as_root() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (ok, said) = kyte(d.path(), &newer_release(), &["--update-check"], "");
    assert!(ok, "{said}");
    assert!(said.contains("Kyte 99.0.0 is out"), "{said}");
    assert!(said.contains("https://example.test/v99"), "{said}");
}

#[test]
fn check_says_when_this_is_the_newest() {
    if as_root() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let answer = br#"{"tag_name":"v0.0.1"}"#.to_vec();
    let url = serve(vec![("latest".to_string(), answer)]);
    let (ok, said) = kyte(d.path(), &url, &["--update"], "");
    assert!(ok, "{said}");
    assert!(said.contains("is the newest release"), "{said}");
}

/// A test build must be refused before anything is asked or downloaded.
#[test]
fn a_build_from_source_wont_update_itself() {
    if as_root() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (ok, said) = kyte(d.path(), &newer_release(), &["--update", "--yes"], "");
    assert!(!ok, "{said}");
    assert!(said.contains("built from source"), "{said}");
    assert!(said.contains("install.sh"), "{said}");
    assert!(!said.contains("Install Kyte"), "{said}");
}

#[test]
fn nothing_to_roll_back_to_is_said_plainly() {
    if as_root() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (ok, said) = kyte(d.path(), &newer_release(), &["--update-rollback"], "");
    assert!(!ok, "{said}");
    assert!(said.contains("nothing to roll back to"), "{said}");
}

#[test]
fn a_version_must_look_like_one() {
    if as_root() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (ok, said) = kyte(d.path(), &newer_release(), &["--update-to", "latest"], "");
    assert!(!ok, "{said}");
    assert!(said.contains("isn't a version"), "{said}");
}

#[test]
fn offline_is_reported_to_someone_who_asked() {
    if as_root() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let (ok, said) = kyte(d.path(), "http://127.0.0.1:1", &["--update-check"], "");
    assert!(!ok, "{said}");
    assert!(said.contains("couldn't reach GitHub"), "{said}");
}

/// The real thing, once: GitHub's TLS and redirects, and the signature the real
/// workflow made, checked with the key built into Kyte. It needs the network,
/// so it only runs when asked: `cargo test --test update -- --ignored`.
#[test]
#[ignore = "needs the network"]
fn a_published_release_passes_every_check() {
    let d = tempfile::tempdir().unwrap();
    let real = Releases {
        latest: format!(
            "https://api.github.com/repos/{}/releases/latest",
            release::REPO
        ),
        mirror: None,
        key: release::built_in_key(),
    };
    assert!(real.latest().unwrap().version >= Version(0, 6, 0));
    let signed = release::Signed::fetch(&real, Version(0, 6, 0), d.path()).unwrap();
    assert!(signed.file("install.sh").unwrap().is_file());
    let tarball = signed.file(&tarball_name()).unwrap();
    let ok = Command::new("tar")
        .arg("-C")
        .arg(d.path())
        .arg("-xzf")
        .arg(&tarball)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let binary = d.path().join(format!("kyte-{}/kyte", target()));
    assert_eq!(version_of(&binary), Some(Version(0, 6, 0)));
}
