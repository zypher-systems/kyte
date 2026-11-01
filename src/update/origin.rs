//! Where this copy of Kyte came from, so `kyte --update` replaces it the same
//! way, or not at all.
//!
//! A package manager's copy stays that package manager's. A package from a
//! repository is updated through that repository, never swapped for the
//! release's own package. A copy from the install script is replaced by the
//! release's install script. A build from source is left alone, wherever it was
//! copied to. Anything that can't be placed is refused, not guessed at.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::release::RELEASE_BUILD;

/// How this copy of Kyte got onto the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// A release binary in a `bin` folder that no package owns: the install
    /// script's work.
    Copied {
        bin: PathBuf,
        /// The folder it's in.
        dir: PathBuf,
    },
    /// Owned by an RPM.
    Rpm {
        package: String,
        origin: Origin<String>,
    },
    /// Owned by a Debian package.
    Deb {
        package: String,
        origin: Origin<AptRepo>,
    },
    /// Owned by a pacman package.
    Pacman { package: String },
    /// Built from source, in a checkout, by `cargo install`, or copied from
    /// either.
    Built(PathBuf),
    /// Somewhere no installer puts it.
    Unknown(PathBuf),
}

/// Where a package came from, as far as its package manager can say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin<R> {
    /// A repository.
    Repo(R),
    /// A package file installed by hand.
    ByHand,
    /// The package manager couldn't say. That isn't a hand install:
    /// `kyte --update` doesn't replace what it can't place.
    Unknown(String),
}

/// The apt repository a package came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AptRepo {
    /// Its host, or its address when it has none (`file:/srv/repo`).
    pub name: String,
    /// The versions it offers (`0.7.0-1`), newest first, as of the last
    /// `apt-get update`. Other repositories' versions aren't here.
    pub versions: Vec<String>,
    /// Those of them another repository offers too.
    pub shared: Vec<String>,
}

/// Folders that belong to the package manager. A copy here with no owner means
/// the lookup failed or someone put it there by hand; neither is Kyte's to
/// overwrite.
const PACKAGE_FOLDERS: [&str; 4] = ["/usr/bin", "/usr/sbin", "/bin", "/sbin"];

/// Works out how `bin`, a real path, was installed. `cargo_bin` is cargo's
/// folder of installed programs, and `owner` asks the package managers.
pub fn classify(
    bin: &Path,
    cargo_bin: Option<&Path>,
    release_build: bool,
    owner: impl Fn(&Path) -> Option<Install>,
) -> Install {
    if is_dev_build(bin) || cargo_bin.is_some_and(|b| bin.starts_with(b)) {
        return Install::Built(bin.into());
    }
    if let Some(package) = owner(bin) {
        return package;
    }
    // The path can't tell a release from a build copied into ~/.local/bin.
    if !release_build {
        return Install::Built(bin.into());
    }
    let Some(dir) = bin.parent() else {
        return Install::Unknown(bin.into());
    };
    // The install script only writes `kyte`, so a copy with any other name
    // would see a different file replaced.
    let named = bin.file_name().is_some_and(|n| n == "kyte");
    let in_bin = dir.file_name().is_some_and(|n| n == "bin");
    if !named || !in_bin || PACKAGE_FOLDERS.iter().any(|f| dir == Path::new(f)) {
        return Install::Unknown(bin.into());
    }
    Install::Copied {
        bin: bin.into(),
        dir: dir.into(),
    }
}

/// How this machine installed `bin`.
pub fn detect(bin: &Path) -> Install {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .map(|h| PathBuf::from(h).join(".cargo"))
        });
    let cargo_bin = cargo_home.map(|c| {
        let bin = c.join("bin");
        bin.canonicalize().unwrap_or(bin)
    });
    classify(bin, cargo_bin.as_deref(), RELEASE_BUILD, package_owner)
}

/// A binary cargo built in a checkout: `…/target/[<triple>/]{debug,release}/`.
pub fn is_dev_build(exe: &Path) -> bool {
    let parts: Vec<String> = exe
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.iter().enumerate().any(|(i, p)| {
        p == "target"
            && parts[i + 1..]
                .iter()
                .take(2)
                .any(|q| q == "debug" || q == "release")
    })
}

/// What `prog args…` prints, when it runs and succeeds.
fn output_of(prog: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The first line `prog args… path` prints.
fn first_line(prog: &str, args: &[&str], path: &str) -> Option<String> {
    let mut args = args.to_vec();
    args.push(path);
    let out = output_of(prog, &args)?;
    let first = out.lines().next()?.trim();
    (!first.is_empty()).then(|| first.to_string())
}

/// Asks rpm, then dpkg, then pacman who owns `path`, and the package manager
/// where the package came from.
fn package_owner(path: &Path) -> Option<Install> {
    let path = path.to_string_lossy();
    if let Some(package) = first_line("rpm", &["-qf", "--qf", "%{NAME}\\n"], &path) {
        let origin = rpm_origin(&package);
        return Some(Install::Rpm { package, origin });
    }
    // `kyte: /usr/bin/kyte`
    if let Some(found) = first_line("dpkg", &["-S"], &path) {
        if let Some((package, _)) = found.split_once(':') {
            let package = package.trim().to_string();
            let origin = apt_origin(&package);
            return Some(Install::Deb { package, origin });
        }
    }
    first_line("pacman", &["-Qoq"], &path).map(|package| Install::Pacman { package })
}

/// Where an installed RPM came from, from dnf's own record and its cache, so
/// no network. Neither dnf answering is unknown, not by hand.
fn rpm_origin(package: &str) -> Origin<String> {
    let query = [
        "-q",
        "-C",
        "repoquery",
        "--installed",
        "--queryformat",
        "%{from_repo}\\n",
        package,
    ];
    ["dnf5", "dnf"]
        .iter()
        .find_map(|dnf| output_of(dnf, &query))
        .map_or_else(
            || Origin::Unknown("neither dnf5 nor dnf could say where it came from".into()),
            |out| dnf_origin(&out),
        )
}

/// Reads dnf's `%{from_repo}`: a repository id, `@commandline` for a file
/// installed by hand, and anything else unknown (`@System`, `<unknown>`,
/// nothing).
pub fn dnf_origin(out: &str) -> Origin<String> {
    let is_id = |s: &str| {
        !s.starts_with('@')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c))
    };
    match out.lines().map(str::trim).find(|l| !l.is_empty()) {
        Some("@commandline") => Origin::ByHand,
        Some(repo) if is_id(repo) => Origin::Repo(repo.to_string()),
        Some(other) => Origin::Unknown(format!("dnf records its origin as {other}")),
        None => Origin::Unknown("dnf has no record of where it came from".into()),
    }
}

/// Where an installed package is available from, from `apt-cache policy`.
fn apt_origin(package: &str) -> Origin<AptRepo> {
    match output_of("apt-cache", &["policy", package]) {
        Some(out) => apt_policy(&out),
        None => Origin::Unknown("apt-cache couldn't say where it came from".into()),
    }
}

/// One line of apt's version table.
struct AptVersion {
    version: String,
    /// The line apt marks `***`.
    installed: bool,
    /// The repositories offering it, by address.
    sources: Vec<String>,
}

/// A priority in apt's table, which a pin can make negative.
fn is_priority(word: &str) -> bool {
    let digits = word.strip_prefix('-').unwrap_or(word);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Reads `apt-cache policy <package>`, keeping each version with the
/// repositories that offer it. Any source with an address is a repository,
/// `file:` and `copy:` included; `/var/lib/dpkg/status` only says it's
/// installed.
///
/// No repository is a `.deb` installed by hand, and no table at all is unknown.
/// One repository is where the package came from. Of several, it's the one that
/// offers the installed version, and if that isn't exactly one, the origin
/// can't be told.
pub fn apt_policy(text: &str) -> Origin<AptRepo> {
    let mut table = text
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("Version table:"))
        .peekable();
    if table.peek().is_none() {
        return Origin::Unknown("apt-cache doesn't list the package".into());
    }
    let mut versions: Vec<AptVersion> = Vec::new();
    for line in table {
        let line = line.trim_start();
        let installed = line.starts_with("***");
        let mut words = line.trim_start_matches("***").split_whitespace();
        let (Some(first), Some(second)) = (words.next(), words.next()) else {
            continue;
        };
        if is_priority(second) {
            // `0.7.0-1 500`
            versions.push(AptVersion {
                version: first.to_string(),
                installed,
                sources: Vec::new(),
            });
        } else if is_priority(first) && second != "/var/lib/dpkg/status" {
            // `500 https://host/path suite/component arch Packages`
            if !second.contains(':') {
                return Origin::Unknown(format!("apt lists an unknown source, {second}"));
            }
            match versions.last_mut() {
                Some(v) if !v.sources.iter().any(|s| s == second) => {
                    v.sources.push(second.to_string())
                }
                _ => {}
            }
        }
    }
    let mut repos: Vec<&str> = Vec::new();
    for source in versions.iter().flat_map(|v| &v.sources) {
        if !repos.contains(&source.as_str()) {
            repos.push(source);
        }
    }
    let home = match repos.as_slice() {
        [] => return Origin::ByHand,
        [only] => only.to_string(),
        _ => match versions
            .iter()
            .find(|v| v.installed)
            .map(|v| v.sources.as_slice())
        {
            Some([home]) => home.clone(),
            _ => {
                return Origin::Unknown(
                    "several repositories offer it, and apt can't say which one this is from"
                        .into(),
                )
            }
        },
    };
    let ours = || versions.iter().filter(|v| v.sources.contains(&home));
    Origin::Repo(AptRepo {
        name: home
            .split_once("://")
            .map_or(home.as_str(), |(_, rest)| {
                rest.split('/').next().unwrap_or(rest)
            })
            .to_string(),
        versions: ours().map(|v| v.version.clone()).collect(),
        shared: ours()
            .filter(|v| v.sources.len() > 1)
            .map(|v| v.version.clone())
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nobody(_: &Path) -> Option<Install> {
        None
    }

    fn rpm(_: &Path) -> Option<Install> {
        Some(Install::Rpm {
            package: "kyte".into(),
            origin: Origin::ByHand,
        })
    }

    #[test]
    fn a_release_in_a_bin_folder_came_from_the_installer() {
        for (bin, dir) in [
            ("/home/me/.local/bin/kyte", "/home/me/.local/bin"),
            ("/usr/local/bin/kyte", "/usr/local/bin"),
            ("/home/me/bin/kyte", "/home/me/bin"),
        ] {
            assert_eq!(
                classify(Path::new(bin), None, true, nobody),
                Install::Copied {
                    bin: bin.into(),
                    dir: dir.into()
                }
            );
        }
    }

    #[test]
    fn a_build_from_source_is_never_a_release() {
        let cargo = Path::new("/home/me/.cargo/bin");
        for p in [
            "/home/me/kyte/target/release/kyte",
            "/home/me/kyte/target/debug/kyte",
            "/home/me/kyte/target/x86_64-unknown-linux-musl/release/kyte",
            "/home/me/.cargo/bin/kyte",
        ] {
            assert_eq!(
                classify(Path::new(p), Some(cargo), true, rpm),
                Install::Built(p.into()),
                "even if something claims to own it"
            );
        }
        // Cargo's folder moved with CARGO_HOME.
        assert_eq!(
            classify(
                Path::new("/opt/cargo/bin/kyte"),
                Some(Path::new("/opt/cargo/bin")),
                true,
                nobody
            ),
            Install::Built("/opt/cargo/bin/kyte".into())
        );
    }

    /// The path alone can't catch this: only the release workflow's mark can.
    #[test]
    fn a_build_copied_into_place_is_not_a_release() {
        let p = "/home/me/.local/bin/kyte";
        assert_eq!(
            classify(Path::new(p), None, false, nobody),
            Install::Built(p.into())
        );
    }

    #[test]
    fn a_target_folder_with_another_name_is_not_a_dev_build() {
        assert!(!is_dev_build(Path::new("/home/me/target/bin/kyte")));
        assert!(!is_dev_build(Path::new("/x/target-other/release/kyte")));
        assert!(is_dev_build(Path::new("/x/target/release/kyte")));
    }

    #[test]
    fn a_package_stays_its_package_managers() {
        assert!(matches!(
            classify(Path::new("/usr/bin/kyte"), None, true, rpm),
            Install::Rpm { .. }
        ));
        // A distribution's own build has no release mark, and is still a package.
        assert!(matches!(
            classify(Path::new("/usr/bin/kyte"), None, false, rpm),
            Install::Rpm { .. }
        ));
    }

    #[test]
    fn a_package_folder_with_no_owner_is_refused() {
        for p in ["/usr/bin/kyte", "/bin/kyte", "/usr/sbin/kyte"] {
            assert_eq!(
                classify(Path::new(p), None, true, nobody),
                Install::Unknown(p.into()),
                "a failed lookup must not read as the installer's copy"
            );
        }
    }

    #[test]
    fn somewhere_no_installer_puts_it_is_refused() {
        for p in ["/opt/tools/kyte", "/home/me/Downloads/kyte", "/kyte"] {
            assert_eq!(
                classify(Path::new(p), None, true, nobody),
                Install::Unknown(p.into())
            );
        }
    }

    /// The install script only ever writes `kyte`, so any other name in that
    /// folder would see a different file replaced.
    /// One repository's several suites are still one repository.
    #[test]
    fn apt_counts_a_repository_once() {
        let policy = "kyte:\n  Version table:\n     0.7.0-1 500\n        500 https://a.test/d \
                      stable/main amd64 Packages\n        500 https://a.test/d stable-updates/main \
                      amd64 Packages\n *** 0.6.0-1 100\n        100 /var/lib/dpkg/status\n";
        let Origin::Repo(repo) = apt_policy(policy) else {
            panic!("one repository");
        };
        assert_eq!(repo.versions, ["0.7.0-1"]);
        assert!(repo.shared.is_empty());
    }

    const TWO_REPOSITORIES: &str = "kyte:\n  Version table:\n     0.8.0-1 500\n        500 \
        https://a.test/d stable/main amd64 Packages\n *** 0.7.0-1 500\n        500 https://a.test/d \
        stable/main amd64 Packages\n        100 /var/lib/dpkg/status\n     0.6.0-1 500\n        500 \
        https://b.test/d stable/main amd64 Packages\n";

    /// Another repository's version must not read as this one's: going to
    /// 0.6.0 here would take a version from b.test and book it to a.test.
    #[test]
    fn apt_keeps_each_version_with_its_repository() {
        assert_eq!(
            apt_policy(TWO_REPOSITORIES),
            Origin::Repo(AptRepo {
                name: "a.test".into(),
                versions: vec!["0.8.0-1".into(), "0.7.0-1".into()],
                shared: Vec::new(),
            })
        );
    }

    #[test]
    fn a_version_two_repositories_offer_is_marked() {
        let policy = TWO_REPOSITORIES.replace(
            "0.8.0-1 500\n        500 https://a.test/d stable/main amd64 Packages\n",
            "0.8.0-1 500\n        500 https://a.test/d stable/main amd64 Packages\n        \
             500 https://b.test/d stable/main amd64 Packages\n",
        );
        let Origin::Repo(repo) = apt_policy(&policy) else {
            panic!("the installed version says which");
        };
        assert_eq!(repo.name, "a.test");
        assert_eq!(repo.shared, ["0.8.0-1"]);
    }

    #[test]
    fn several_repositories_and_no_way_to_choose_is_unknown() {
        // Neither offers the installed version any more.
        let gone = "kyte:\n  Version table:\n     0.8.0-1 500\n        500 https://a.test/d s/m \
                    amd64 Packages\n     0.7.0-1 500\n        500 https://b.test/d s/m amd64 \
                    Packages\n *** 0.6.0-1 100\n        100 /var/lib/dpkg/status\n";
        assert!(matches!(apt_policy(gone), Origin::Unknown(_)));
        // Both offer it.
        let both = "kyte:\n  Version table:\n *** 0.7.0-1 500\n        500 https://a.test/d s/m \
                    amd64 Packages\n        500 https://b.test/d s/m amd64 Packages\n        100 \
                    /var/lib/dpkg/status\n";
        assert!(matches!(apt_policy(both), Origin::Unknown(_)));
    }

    /// A local repository is written without `//`.
    #[test]
    fn a_local_apt_repository_is_a_repository() {
        let policy = "kyte:\n  Version table:\n     0.7.0-1 500\n        500 file:/srv/repo ./ \
                      Packages\n *** 0.6.0-1 100\n        100 /var/lib/dpkg/status\n";
        assert_eq!(
            apt_policy(policy),
            Origin::Repo(AptRepo {
                name: "file:/srv/repo".into(),
                versions: vec!["0.7.0-1".into()],
                shared: Vec::new(),
            })
        );
    }

    #[test]
    fn a_source_apt_cant_describe_is_unknown() {
        let policy = "kyte:\n  Version table:\n *** 0.6.0-1 500\n        500 /some/where\n";
        assert!(matches!(apt_policy(policy), Origin::Unknown(_)));
    }

    /// A pin can make a priority negative.
    #[test]
    fn apt_reads_a_negative_priority() {
        let policy = "kyte:\n  Version table:\n     0.7.0-1 -10\n        -10 https://a.test/d s/m \
                      amd64 Packages\n *** 0.6.0-1 100\n        100 /var/lib/dpkg/status\n";
        let Origin::Repo(repo) = apt_policy(policy) else {
            panic!("a repository");
        };
        assert_eq!(repo.versions, ["0.7.0-1"]);
    }

    #[test]
    fn a_copy_under_another_name_is_refused() {
        for p in ["/home/me/.local/bin/kyte-old", "/usr/local/bin/kyte.bak"] {
            assert_eq!(
                classify(Path::new(p), None, true, nobody),
                Install::Unknown(p.into())
            );
        }
    }

    #[test]
    fn dnf_says_where_a_package_came_from() {
        assert_eq!(dnf_origin("copr-kyte\n"), Origin::Repo("copr-kyte".into()));
        assert_eq!(dnf_origin("\nfedora\n\n"), Origin::Repo("fedora".into()));
        assert_eq!(dnf_origin("@commandline\n"), Origin::ByHand);
        for unknown in ["@System", "<unknown>", "", "\n"] {
            assert!(
                matches!(dnf_origin(unknown), Origin::Unknown(_)),
                "{unknown:?}: no record isn't a hand install"
            );
        }
    }

    #[test]
    fn apt_says_where_a_package_came_from() {
        let policy = "kyte:\n  Installed: 0.6.0-1\n  Candidate: 0.7.0-1\n  Version table:\n     \
                      0.7.0-1 500\n        500 https://repo.example.org/debian stable/main amd64 Packages\n \
                      *** 0.6.0-1 100\n        100 /var/lib/dpkg/status\n     0.5.2-1 500\n        500 \
                      https://repo.example.org/debian stable/main amd64 Packages\n";
        assert_eq!(
            apt_policy(policy),
            Origin::Repo(AptRepo {
                name: "repo.example.org".into(),
                versions: vec!["0.7.0-1".into(), "0.5.2-1".into()],
                shared: Vec::new(),
            }),
            "the installed version is local only; the other two are the repository's"
        );
        let by_hand = "kyte:\n  Installed: 0.6.0-1\n  Candidate: 0.6.0-1\n  Version table:\n \
                       *** 0.6.0-1 100\n        100 /var/lib/dpkg/status\n";
        assert_eq!(apt_policy(by_hand), Origin::ByHand);
        assert!(matches!(
            apt_policy("N: Unable to locate package kyte\n"),
            Origin::Unknown(_)
        ));
    }
}
