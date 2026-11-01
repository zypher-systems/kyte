//! `kyte --update`: installs another version the way this one was installed.
//!
//! - Copied by the install script: download the release's tarball and install
//!   script, check them, and run that release's installer over this copy.
//! - An RPM or .deb from a repository: updated through that repository, by
//!   version, so the release's own package never replaces it. If the repository
//!   doesn't have the version, nothing changes.
//! - An RPM or .deb installed by hand: download the release's, check it, and
//!   hand it to dnf or apt-get.
//! - A pacman package: handed to the AUR helper.
//! - A build from source, or a copy that can't be placed: refused, with what to
//!   do instead.
//!
//! Going back installs the version the last update replaced, from its own
//! release, checked the same way.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::origin::{self, Install, Origin};
use super::release::{self, Releases, Signed, State, Version, REPO};
use super::{Action, Request};
use crate::fileio;

/// The install script, for when Kyte can't update itself.
fn install_line() -> String {
    format!("curl -fsSL https://raw.githubusercontent.com/{REPO}/main/install.sh | sh")
}

/// How long a binary has to report its version.
const PROBE_LIMIT: Duration = Duration::from_secs(10);

/// How much of its answer is read.
const PROBE_OUTPUT: usize = 4 << 10;

/// How the new version gets in.
#[derive(Debug, PartialEq, Eq)]
pub enum Kind {
    /// The release's install script, from its verified tarball.
    Installer {
        /// The folder this copy is in.
        into: PathBuf,
        /// This user can't write there.
        sudo: bool,
    },
    /// The release RPM, through dnf.
    Rpm { dnf: String, downgrade: bool },
    /// The release .deb, through apt-get.
    Deb { downgrade: bool },
    /// The repository the package came from, through its package manager,
    /// which is run with sudo.
    Repo { command: Vec<String>, repo: String },
    /// The AUR helper.
    Aur { helper: String, package: String },
}

/// What will happen, before anyone is asked.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub describe: String,
    pub kind: Kind,
}

/// The package tools on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tools {
    /// `dnf5` or `dnf`.
    pub dnf: String,
    /// An image-based system, which layers packages with rpm-ostree.
    pub atomic: bool,
    /// `paru` or `yay`.
    pub aur: Option<String>,
}

impl Tools {
    fn detect() -> Self {
        let on_path = |name: &str| {
            std::env::var_os("PATH")
                .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
        };
        Self {
            dnf: if on_path("dnf5") { "dnf5" } else { "dnf" }.into(),
            atomic: Path::new("/run/ostree-booted").exists(),
            aur: ["paru", "yay"]
                .iter()
                .find(|h| on_path(h))
                .map(|h| h.to_string()),
        }
    }
}

/// Works out how to get from `running` to `to`, or why this copy can't be
/// updated. `latest`: `to` is the newest release, not a version someone chose.
pub fn plan(
    install: &Install,
    running: Version,
    to: Version,
    latest: bool,
    releases: &Releases,
    tools: &Tools,
) -> Result<Plan, String> {
    let head = format!("Kyte {running} → {to}");
    let downgrade = to < running;
    let base = releases.base(to);
    match install {
        Install::Built(p) => Err(format!(
            "{} was built from source, not installed from a release, so `kyte --update` won't \
             replace it. Update the checkout and build it again, or install the release: {}",
            p.display(),
            install_line()
        )),
        Install::Unknown(p) => Err(format!(
            "{} isn't where an installer puts Kyte, so `kyte --update` won't replace it. \
             Reinstall it with the install script, or replace the binary yourself: {}",
            p.display(),
            install_line()
        )),
        Install::Copied { bin, dir } => {
            let sudo = rustix::fs::access(dir, rustix::fs::Access::WRITE_OK).is_err();
            let how = if sudo {
                ", using sudo to put it in place"
            } else {
                ""
            };
            Ok(Plan {
                describe: format!(
                    "{head}\nInstalled by the install script.\nDownload the release from {base}, \
                     check its signature and checksums, and run its installer over {}{how}.",
                    bin.display()
                ),
                kind: Kind::Installer {
                    into: dir.clone(),
                    sudo,
                },
            })
        }
        Install::Rpm { package, origin } => {
            if tools.atomic {
                return Err(format!(
                    "{package} is an RPM on an image-based system; update it with rpm-ostree"
                ));
            }
            let dnf = tools.dnf.clone();
            let verb = if downgrade { "downgrade" } else { "upgrade" };
            match origin {
                // A repository's build stays the one installed: the version
                // comes from that repository and no other, or nothing changes.
                Origin::Repo(repo) => {
                    let spec = format!("{package}-{to}");
                    // dnf5 takes the named package from the repository and
                    // dependencies from anywhere; dnf 4 can only narrow to it.
                    let only = if dnf == "dnf5" {
                        format!("--from-repo={repo}")
                    } else {
                        format!("--repo={repo}")
                    };
                    Ok(Plan {
                        describe: format!(
                            "{head}\n{package} comes from the {repo} repository.\nRun: sudo {dnf} \
                             {verb} -y {only} {spec}. If {repo} doesn't have {to} yet, nothing \
                             changes."
                        ),
                        kind: Kind::Repo {
                            command: vec![dnf, verb.into(), "-y".into(), only, spec],
                            repo: repo.clone(),
                        },
                    })
                }
                Origin::Unknown(why) => Err(format!(
                    "{package} is an RPM, but where it came from couldn't be told ({why}), so \
                     `kyte --update` won't replace it. Update it with {dnf}, or reinstall the \
                     release's RPM yourself."
                )),
                Origin::ByHand => Ok(Plan {
                    describe: format!(
                        "{head}\n{package} is an RPM installed from a file.\nDownload {} from \
                         {base}, check its signature and checksum, then run: sudo {dnf} {verb} -y \
                         <that file>",
                        release::rpm_name(to),
                    ),
                    kind: Kind::Rpm { dnf, downgrade },
                }),
            }
        }
        Install::Deb { package, origin } => {
            let allow = if downgrade { " --allow-downgrades" } else { "" };
            match origin {
                // apt installs an exact version: the repository's build of
                // `to`, whatever revision it gave it.
                Origin::Repo(repo) => {
                    let wanted = to.to_string();
                    let revision = format!("{to}-");
                    let Some(version) = repo
                        .versions
                        .iter()
                        .find(|v| **v == wanted || v.starts_with(&revision))
                    else {
                        let has = if repo.versions.is_empty() {
                            "nothing listed".to_string()
                        } else {
                            repo.versions.join(", ")
                        };
                        return Err(format!(
                            "{package} comes from the {} repository, which doesn't have {to} in \
                             what apt last fetched ({has}). Run `sudo apt-get update` and try \
                             again, or wait for the repository.",
                            repo.name
                        ));
                    };
                    let spec = format!("{package}={version}");
                    let mut command = vec!["apt-get".to_string(), "install".into(), "-y".into()];
                    if downgrade {
                        command.push("--allow-downgrades".into());
                    }
                    command.push(spec.clone());
                    Ok(Plan {
                        describe: format!(
                            "{head}\n{package} comes from the {} repository.\nRun: sudo apt-get \
                             install -y{allow} {spec}",
                            repo.name
                        ),
                        kind: Kind::Repo {
                            command,
                            repo: repo.name.clone(),
                        },
                    })
                }
                Origin::Unknown(why) => Err(format!(
                    "{package} is a Debian package, but where it came from couldn't be told \
                     ({why}), so `kyte --update` won't replace it. Update it with apt, or \
                     reinstall the release's .deb yourself."
                )),
                Origin::ByHand => Ok(Plan {
                    describe: format!(
                        "{head}\n{package} is a Debian package installed from a file.\nDownload \
                         {} from {base}, check its signature and checksum, then run: sudo apt-get \
                         install -y{allow} <that file>",
                        release::deb_name(to),
                    ),
                    kind: Kind::Deb { downgrade },
                }),
            }
        }
        Install::Pacman { package } => {
            if !latest {
                return Err(format!(
                    "{package} comes from pacman, which installs a chosen version from its cache: \
                     ls /var/cache/pacman/pkg/{package}-*, then sudo pacman -U <file>"
                ));
            }
            let Some(helper) = tools.aur.clone() else {
                return Err(format!(
                    "{package} comes from pacman, and there's no AUR helper (paru or yay) to \
                     update it. Update it the way you installed it."
                ));
            };
            Ok(Plan {
                describe: format!(
                    "{head}\n{package} comes from pacman.\nRun: {helper} -S {package}"
                ),
                kind: Kind::Aur {
                    helper,
                    package: package.clone(),
                },
            })
        }
    }
}

/// The command that runs a release's installer over the copy in `into`. The
/// script reads its folder from the environment, and `env` carries it past
/// sudo, which clears the rest.
pub fn installer_command(script: &Path, tarball: &Path, into: &Path, sudo: bool) -> Command {
    let place = format!("KYTE_INSTALL_DIR={}", into.display());
    let mut cmd = if sudo {
        let mut c = Command::new("sudo");
        c.arg("env").arg(&place).arg("sh");
        c
    } else {
        let mut c = Command::new("env");
        c.arg(&place).arg("sh");
        c
    };
    cmd.arg(script).arg("--from").arg(tarball);
    cmd
}

/// Puts `to` in place. `work` is a private folder for the downloads.
pub fn execute(
    kind: &Kind,
    to: Version,
    releases: &Releases,
    work: &Path,
) -> Result<String, String> {
    match kind {
        Kind::Installer { into, sudo } => {
            let target = release::target().ok_or("there's no prebuilt Kyte for this CPU")?;
            let signed = Signed::fetch(releases, to, work)?;
            let tarball = signed.file(&release::tarball_name(target))?;
            let script = signed.file("install.sh")?;
            let member = format!("kyte-{target}/kyte");
            run(
                Command::new("tar")
                    .arg("-C")
                    .arg(work)
                    .arg("-xzf")
                    .arg(&tarball)
                    .arg(&member),
                "tar",
            )?;
            // A signature says the file is Kyte's, not which release it is:
            // an old signed release mustn't pass for a new one.
            let binary = work.join(&member);
            let is_file = fs::symlink_metadata(&binary).is_ok_and(|m| m.is_file());
            match is_file.then(|| version_of(&binary)).flatten() {
                Some(v) if v == to => {}
                v => {
                    return Err(format!(
                        "the {to} release holds {}; nothing was installed",
                        v.map_or("no working kyte".into(), |v| format!("Kyte {v}"))
                    ));
                }
            }
            run(
                &mut installer_command(&script, &tarball, into, *sudo),
                "the installer",
            )?;
            Ok(format!("Kyte {to} installed in {}", into.display()))
        }
        Kind::Rpm { dnf, downgrade } => {
            let rpm = Signed::fetch(releases, to, work)?.file(&release::rpm_name(to))?;
            let verb = if *downgrade { "downgrade" } else { "upgrade" };
            run(
                Command::new("sudo")
                    .args([dnf.as_str(), verb, "-y"])
                    .arg(&rpm),
                dnf,
            )?;
            Ok(format!("Kyte {to} installed with {dnf}"))
        }
        Kind::Deb { downgrade } => {
            let deb = Signed::fetch(releases, to, work)?.file(&release::deb_name(to))?;
            let mut cmd = Command::new("sudo");
            cmd.args(["apt-get", "install", "-y"]);
            if *downgrade {
                cmd.arg("--allow-downgrades");
            }
            run(cmd.arg(&deb), "apt-get")?;
            Ok(format!("Kyte {to} installed with apt-get"))
        }
        Kind::Repo { command, repo } => {
            let tool = command
                .first()
                .map_or("the package manager", String::as_str);
            run(Command::new("sudo").args(command), tool).map_err(|e| {
                format!("{e}; the {repo} repository may not have Kyte {to} yet, in which case nothing changed")
            })?;
            Ok(format!(
                "Kyte {to} installed from the {repo} repository with {tool}"
            ))
        }
        Kind::Aur { helper, package } => {
            run(Command::new(helper).args(["-S", package.as_str()]), helper)?;
            Ok(format!("{package} reinstalled with {helper}"))
        }
    }
}

/// Runs with the terminal attached, because sudo and the installers talk to you.
fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let status = cmd
        .status()
        .map_err(|e| format!("couldn't run {what}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed (its output is above)"))
    }
}

/// What `binary --version` says.
pub fn version_of(binary: &Path) -> Option<Version> {
    let said = probe(binary, PROBE_LIMIT)?;
    let mut words = said.split_whitespace();
    (words.next() == Some("kyte"))
        .then(|| words.next())
        .flatten()
        .and_then(Version::parse)
}

/// What `binary --version` prints within `limit`.
///
/// Nothing it does can hold the update up. It gets no input, so it can't read
/// the terminal. Its output is read up to [`PROBE_OUTPUT`] by a reader that's
/// never waited on, so a flood ends when that reader stops, and a child left
/// holding the output open doesn't block. Past `limit`, it and everything it
/// started, its own process group, are killed and reaped.
fn probe(binary: &Path, limit: Duration) -> Option<String> {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(binary);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    // "Text file busy": a process another thread started just now can hold the
    // file open for a few milliseconds, until it has loaded its own program.
    let busy_until = Instant::now() + limit.min(Duration::from_secs(2));
    let mut child = loop {
        match cmd.spawn() {
            Ok(child) => break child,
            Err(e)
                if e.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && Instant::now() < busy_until =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    };
    let said = Arc::new(Mutex::new(Vec::new()));
    let ended = Arc::new(AtomicBool::new(false));
    if let Some(mut out) = child.stdout.take() {
        let (said, ended) = (said.clone(), ended.clone());
        std::thread::spawn(move || {
            let mut chunk = [0u8; 1024];
            while let Ok(n @ 1..) = out.read(&mut chunk) {
                let Ok(mut said) = said.lock() else { break };
                said.extend_from_slice(&chunk[..n]);
                if said.len() >= PROBE_OUTPUT {
                    break;
                }
            }
            ended.store(true, Ordering::Release);
        });
    }
    let until = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                // Killed while its leader is unreaped, so the id can't have
                // passed to another process.
                let group = rustix::process::Pid::from_child(&child);
                let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    // Its output is in the pipe by now, but a child of its own may hold the
    // pipe open, so the end isn't waited for long.
    let grace = Instant::now() + Duration::from_millis(500);
    while !ended.load(Ordering::Acquire) && Instant::now() < grace {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !status.success() {
        return None;
    }
    let said = said.lock().ok()?;
    Some(String::from_utf8_lossy(&said[..said.len().min(PROBE_OUTPUT)]).into_owned())
}

/// Asks yes or no on the terminal. Anything but yes is no, and so is no input.
fn confirm(question: &str) -> Result<bool, String> {
    print!("{question}");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|e| e.to_string())?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// A fresh private folder for the downloads. They're run later, so nobody else
/// gets to touch them first.
fn fresh_folder(dir: &Path) -> Result<PathBuf, String> {
    let at = |e: std::io::Error| format!("{}: {e}", dir.display());
    let dir = std::path::absolute(dir).map_err(at)?;
    let _ = fs::remove_dir_all(&dir);
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).map_err(at)?;
    }
    fs::DirBuilder::new().mode(0o700).create(&dir).map_err(at)?;
    Ok(dir)
}

/// Does what `request` asks.
pub fn run_request(request: &Request) -> Result<(), String> {
    // Its state and downloads belong to the person, and a root process would
    // leave them owned by root.
    if rustix::process::geteuid().is_root() {
        return Err("run `kyte --update` as yourself; it asks for sudo when it needs it".into());
    }
    let releases = Releases::default();
    let running = Version::current();
    let dir = fileio::update_dir();

    let mut notes = None;
    let to = match &request.action {
        Action::Rollback => State::load(&dir)
            .previous
            .as_deref()
            .and_then(Version::parse)
            .ok_or("nothing to roll back to: `kyte --update` hasn't replaced a version yet")?,
        Action::To(v) => {
            Version::parse(v).ok_or_else(|| format!("{v:?} isn't a version like 0.6.0"))?
        }
        Action::Latest | Action::Check => {
            let found = releases.latest()?;
            let _ = State::modify(&dir, |s| {
                s.checked = Some(release::now());
                s.latest = Some(found.version.to_string());
            });
            if found.version <= running {
                println!("Kyte {running} is the newest release.");
                return Ok(());
            }
            notes = (!found.url.is_empty()).then_some(found.url);
            found.version
        }
    };
    if request.action == Action::Check {
        println!("Kyte {to} is out; this is {running}. `kyte --update` installs it.");
        if let Some(url) = &notes {
            println!("What's new: {url}");
        }
        return Ok(());
    }
    if to == running {
        println!("This is already Kyte {to}.");
        return Ok(());
    }

    let bin = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("can't tell where this kyte is: {e}"))?;
    let install = origin::detect(&bin);
    let latest = request.action == Action::Latest;
    let plan = plan(&install, running, to, latest, &releases, &Tools::detect())?;
    println!("{}", plan.describe);
    if let Some(url) = &notes {
        println!("What's new: {url}");
    }
    if !request.yes && !confirm(&format!("Install Kyte {to}? [y/N] "))? {
        return Ok(());
    }

    let work = fresh_folder(&dir.join("work"))?;
    let outcome = execute(&plan.kind, to, &releases, &work);
    let _ = fs::remove_dir_all(&work);

    // What's on disk decides what happened, whatever the installer said: a
    // failed install may already have replaced the binary, and then
    // --update-rollback must still know what to put back.
    let after = version_of(&bin);
    let changed = after.is_some_and(|v| v != running);
    if changed {
        let _ = State::modify(&dir, |s| {
            s.previous = Some(running.to_string());
            s.installed = after.map(|v| v.to_string());
        });
    }
    let aur = matches!(plan.kind, Kind::Aur { .. });
    let back = if aur {
        format!("To go back, install {running} from the pacman cache with sudo pacman -U <file>.")
    } else {
        format!("`kyte --update-rollback` goes back to {running}.")
    };
    let finished = match (outcome, after) {
        (Err(e), _) => Err(e),
        (Ok(summary), Some(v)) if v == to => Ok(summary),
        // The AUR's package may trail the release.
        (Ok(_), Some(v)) if aur && v == running => Err(format!(
            "the AUR package is still {running}; it hasn't caught up with {to} yet"
        )),
        (Ok(summary), Some(v)) if aur => Ok(format!("{summary}; the AUR package is {v}")),
        (Ok(_), v) => Err(format!(
            "the install finished, but {} reports {}",
            bin.display(),
            v.map_or("no version".into(), |v| v.to_string())
        )),
    };
    match finished {
        Ok(summary) => {
            println!("{summary}");
            println!("{back} Open Kyte windows keep running {running} until you restart them.");
            Ok(())
        }
        Err(e) => {
            if let (true, Some(v)) = (changed, after) {
                eprintln!("{} is now Kyte {v}. {back}", bin.display());
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::origin::AptRepo;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn releases() -> Releases {
        Releases {
            latest: String::new(),
            mirror: None,
            key: None,
        }
    }

    fn tools(dnf: &str) -> Tools {
        Tools {
            dnf: dnf.into(),
            atomic: false,
            aur: None,
        }
    }

    fn make(install: &Install, running: &str, to: &str, tools: &Tools) -> Result<Plan, String> {
        plan(install, v(running), v(to), true, &releases(), tools)
    }

    fn rpm(origin: Origin<String>) -> Install {
        Install::Rpm {
            package: "kyte".into(),
            origin,
        }
    }

    fn deb(origin: Origin<AptRepo>) -> Install {
        Install::Deb {
            package: "kyte".into(),
            origin,
        }
    }

    #[test]
    fn a_build_from_source_is_refused_and_told_what_to_do() {
        let built = Install::Built("/home/me/.local/bin/kyte".into());
        let e = make(&built, "0.6.0", "0.7.0", &tools("dnf5")).unwrap_err();
        assert!(
            e.contains("built from source") && e.contains("install.sh"),
            "{e}"
        );
        let odd = Install::Unknown("/opt/kyte".into());
        let e = make(&odd, "0.6.0", "0.7.0", &tools("dnf5")).unwrap_err();
        assert!(e.contains("isn't where an installer puts Kyte"), "{e}");
    }

    #[test]
    fn a_copy_runs_the_releases_installer_over_itself() {
        let d = tempfile::tempdir().unwrap();
        let install = Install::Copied {
            bin: d.path().join("kyte"),
            dir: d.path().into(),
        };
        let p = make(&install, "0.6.0", "0.7.0", &tools("dnf5")).unwrap();
        assert_eq!(
            p.kind,
            Kind::Installer {
                into: d.path().into(),
                sudo: false
            }
        );
        assert!(p.describe.contains("signature"), "{}", p.describe);
        assert!(!p.describe.contains("sudo"), "{}", p.describe);
    }

    #[test]
    fn a_folder_this_user_cant_write_takes_sudo() {
        let gone = PathBuf::from("/nonexistent/kyte-test/bin");
        let install = Install::Copied {
            bin: gone.join("kyte"),
            dir: gone.clone(),
        };
        let p = make(&install, "0.6.0", "0.7.0", &tools("dnf5")).unwrap();
        assert_eq!(
            p.kind,
            Kind::Installer {
                into: gone,
                sudo: true
            }
        );
        assert!(p.describe.contains("using sudo"), "{}", p.describe);
    }

    #[test]
    fn the_installer_is_told_where_this_copy_lives() {
        let args = |sudo| {
            let cmd = installer_command(
                Path::new("/w/install.sh"),
                Path::new("/w/k.tar.gz"),
                Path::new("/usr/local/bin"),
                sudo,
            );
            let all: Vec<String> = std::iter::once(cmd.get_program())
                .chain(cmd.get_args())
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            all.join(" ")
        };
        let place = "KYTE_INSTALL_DIR=/usr/local/bin sh /w/install.sh --from /w/k.tar.gz";
        assert_eq!(args(false), format!("env {place}"));
        assert_eq!(args(true), format!("sudo env {place}"));
    }

    #[test]
    fn an_rpm_from_a_repository_updates_through_it() {
        let from = rpm(Origin::Repo("copr-kyte".into()));
        let p = make(&from, "0.6.0", "0.7.0", &tools("dnf5")).unwrap();
        assert!(
            p.describe.contains("copr-kyte repository"),
            "{}",
            p.describe
        );
        assert_eq!(
            p.kind,
            Kind::Repo {
                // From that repository and no other.
                command: [
                    "dnf5",
                    "upgrade",
                    "-y",
                    "--from-repo=copr-kyte",
                    "kyte-0.7.0"
                ]
                .map(String::from)
                .to_vec(),
                repo: "copr-kyte".into()
            }
        );
        let p = make(&from, "0.6.0", "0.7.0", &tools("dnf")).unwrap();
        assert!(matches!(
            &p.kind,
            Kind::Repo { command, .. }
                if command == &["dnf", "upgrade", "-y", "--repo=copr-kyte", "kyte-0.7.0"]
        ));
        // Going back goes back through the repository too.
        let p = make(&from, "0.7.0", "0.6.0", &tools("dnf5")).unwrap();
        assert!(matches!(
            &p.kind,
            Kind::Repo { command, .. } if command[1] == "downgrade" && command[4] == "kyte-0.6.0"
        ));
    }

    #[test]
    fn an_rpm_from_a_file_gets_the_releases_own() {
        let p = make(&rpm(Origin::ByHand), "0.6.0", "0.7.0", &tools("dnf5")).unwrap();
        assert_eq!(
            p.kind,
            Kind::Rpm {
                dnf: "dnf5".into(),
                downgrade: false
            }
        );
        assert!(
            p.describe.contains(&release::rpm_name(v("0.7.0"))),
            "{}",
            p.describe
        );
    }

    #[test]
    fn a_package_of_unknown_origin_is_refused() {
        let e = make(
            &rpm(Origin::Unknown("dnf didn't answer".into())),
            "0.6.0",
            "0.7.0",
            &tools("dnf5"),
        )
        .unwrap_err();
        assert!(
            e.contains("couldn't be told") && e.contains("dnf didn't answer"),
            "{e}"
        );
        let e = make(
            &deb(Origin::Unknown("apt-cache didn't answer".into())),
            "0.6.0",
            "0.7.0",
            &tools("dnf5"),
        )
        .unwrap_err();
        assert!(e.contains("couldn't be told"), "{e}");
    }

    #[test]
    fn an_image_based_system_is_left_to_rpm_ostree() {
        let t = Tools {
            atomic: true,
            ..tools("dnf5")
        };
        let e = make(&rpm(Origin::ByHand), "0.6.0", "0.7.0", &t).unwrap_err();
        assert!(e.contains("rpm-ostree"), "{e}");
    }

    #[test]
    fn a_deb_from_a_repository_updates_through_it() {
        let repo = AptRepo {
            name: "repo.example.org".into(),
            versions: vec!["0.7.0-2".into(), "0.5.2-1".into()],
        };
        let from = deb(Origin::Repo(repo));
        let p = make(&from, "0.6.0", "0.7.0", &tools("dnf")).unwrap();
        assert!(matches!(
            &p.kind,
            Kind::Repo { command, .. }
                if command == &["apt-get", "install", "-y", "kyte=0.7.0-2"]
        ));
        // A downgrade is allowed only when going back.
        let p = make(&from, "0.6.0", "0.5.2", &tools("dnf")).unwrap();
        assert!(matches!(
            &p.kind,
            Kind::Repo { command, .. }
                if command == &["apt-get", "install", "-y", "--allow-downgrades", "kyte=0.5.2-1"]
        ));
        // Not in the repository yet: nothing runs, and the reason says what is.
        let e = make(&from, "0.6.0", "0.8.0", &tools("dnf")).unwrap_err();
        assert!(
            e.contains("doesn't have 0.8.0") && e.contains("0.7.0-2, 0.5.2-1"),
            "{e}"
        );
    }

    #[test]
    fn a_deb_from_a_file_gets_the_releases_own() {
        let p = make(&deb(Origin::ByHand), "0.6.0", "0.7.0", &tools("dnf")).unwrap();
        assert_eq!(p.kind, Kind::Deb { downgrade: false });
        assert!(
            p.describe.contains(&release::deb_name(v("0.7.0"))),
            "{}",
            p.describe
        );
    }

    #[test]
    fn pacman_goes_through_the_aur_helper_or_not_at_all() {
        let pac = Install::Pacman {
            package: "kyte-bin".into(),
        };
        let e = make(&pac, "0.6.0", "0.7.0", &tools("dnf")).unwrap_err();
        assert!(e.contains("no AUR helper"), "{e}");
        let with = Tools {
            aur: Some("paru".into()),
            ..tools("dnf")
        };
        let p = make(&pac, "0.6.0", "0.7.0", &with).unwrap();
        assert_eq!(
            p.kind,
            Kind::Aur {
                helper: "paru".into(),
                package: "kyte-bin".into()
            }
        );
        // A chosen version isn't the AUR's to give.
        let e = plan(&pac, v("0.6.0"), v("0.5.0"), false, &releases(), &with).unwrap_err();
        assert!(e.contains("cache"), "{e}");
    }

    /// A script that says `text` when run.
    fn script(dir: &Path, text: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("fake");
        fs::write(&p, text).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn a_binary_says_its_version_or_has_none() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(
            version_of(&script(d.path(), "#!/bin/sh\necho 'kyte 9.9.9'\n")),
            Some(v("9.9.9"))
        );
        for bad in [
            "#!/bin/sh\necho 'other 9.9.9'\n",
            "#!/bin/sh\necho 'kyte nine'\n",
            "#!/bin/sh\necho 'kyte 9.9.9'\nexit 3\n",
        ] {
            assert_eq!(version_of(&script(d.path(), bad)), None, "{bad}");
        }
        assert_eq!(version_of(&d.path().join("missing")), None);
    }

    /// A signed binary that hangs or floods can't hold the update up.
    #[test]
    fn a_binary_that_hangs_gives_no_answer_in_time() {
        let d = tempfile::tempdir().unwrap();
        let limit = Duration::from_millis(500);
        for bad in [
            "#!/bin/sh\nsleep 30 & sleep 30\n",
            "#!/bin/sh\nwhile :; do echo 'kyte 9.9.9 and more'; done\n",
        ] {
            let started = Instant::now();
            assert_eq!(probe(&script(d.path(), bad), limit), None, "{bad}");
            assert!(started.elapsed() < Duration::from_secs(5), "{bad}");
        }
        // One whose child holds the output open still answers.
        let keeps = script(d.path(), "#!/bin/sh\nsleep 3 &\necho 'kyte 9.9.9'\n");
        let started = Instant::now();
        assert_eq!(
            probe(&keeps, limit).as_deref().map(str::trim),
            Some("kyte 9.9.9")
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn the_download_folder_is_private_and_starts_empty() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let work = d.path().join("update").join("work");
        let made = fresh_folder(&work).unwrap();
        fs::write(made.join("left"), "x").unwrap();
        let made = fresh_folder(&work).unwrap();
        assert!(!made.join("left").exists());
        assert_eq!(
            fs::metadata(&made).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
