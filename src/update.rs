//! Keeping Kyte up to date: `kyte --update`, and a check at launch that only
//! ever says a release is out. This is the only code in Kyte that uses the
//! network, and a build without the `update` feature doesn't have any of it.

use std::process::ExitCode;
use std::sync::mpsc::Receiver;

#[cfg(feature = "update")]
pub mod apply;
#[cfg(feature = "update")]
pub mod origin;
#[cfg(feature = "update")]
pub mod release;

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Install the newest release.
    Latest,
    /// Only say whether a newer release is out.
    Check,
    /// Install this version.
    To(String),
    /// Go back to the version the last update replaced.
    Rollback,
}

/// One run of the updater.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub action: Action,
    /// Don't ask.
    pub yes: bool,
}

/// Does what `request` asks, for `main`.
#[cfg(feature = "update")]
pub fn run(request: &Request) -> ExitCode {
    match apply::run_request(request) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kyte: {e}");
            ExitCode::from(1)
        }
    }
}

/// This build has no updater, and no network code to run one with.
#[cfg(not(feature = "update"))]
pub fn run(_request: &Request) -> ExitCode {
    eprintln!("kyte: this build has no updater; update it the way you built it");
    ExitCode::from(1)
}

/// Starts the launch check beside the editor, so a slow network can't slow
/// Kyte down. A note comes back when there's something to say. Debug builds and
/// builds in a checkout never ask, which keeps the tests off the network.
#[cfg(feature = "update")]
pub fn start_check(enabled: bool) -> Option<Receiver<String>> {
    if !enabled
        || cfg!(debug_assertions)
        || std::env::current_exe().is_ok_and(|e| origin::is_dev_build(&e))
    {
        return None;
    }
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let note = release::on_launch(
                &crate::fileio::update_dir(),
                &release::Releases::default(),
                release::Version::current(),
                release::now(),
            );
            if let Some(note) = note {
                let _ = send.send(note);
            }
        })
        .ok()?;
    Some(receive)
}

/// There's nothing to check with.
#[cfg(not(feature = "update"))]
pub fn start_check(_enabled: bool) -> Option<Receiver<String>> {
    None
}

/// Asks now, because someone chose to. The answer always comes back.
#[cfg(feature = "update")]
pub fn check_now() -> Receiver<String> {
    let (send, receive) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("update-check".into())
        .spawn({
            let send = send.clone();
            move || {
                let note = release::on_request(
                    &crate::fileio::update_dir(),
                    &release::Releases::default(),
                    release::Version::current(),
                    release::now(),
                );
                let _ = send.send(note);
            }
        });
    if let Err(e) = spawned {
        let _ = send.send(format!("Couldn't check for updates: {e}"));
    }
    receive
}

/// This build can't ask.
#[cfg(not(feature = "update"))]
pub fn check_now() -> Receiver<String> {
    let (send, receive) = std::sync::mpsc::channel();
    let _ = send.send("This build of Kyte has no updater".to_string());
    receive
}
