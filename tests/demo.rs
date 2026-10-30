//! AC-25: the headless demo script. Ignored by default because it takes about
//! 40 seconds and rebuilds in release mode; run it with
//! `cargo test --test demo -- --ignored` (or just `make demo`).

use std::path::Path;
use std::process::Command;

#[test]
#[ignore = "slow: runs scripts/demo.sh (about 40 s); use `make demo` or --ignored"]
fn ac25_demo_script_writes_cast_frames_and_media() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = tempfile::tempdir().unwrap();
    let status = Command::new(root.join("scripts/demo.sh"))
        .env("DEMO_OUT", out.path())
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());
    let o = out.path();
    assert!(o.join("demo.cast").metadata().unwrap().len() > 0);
    assert!(o.join("steps.tsv").exists());
    let frame =
        |n: &str| std::fs::read_to_string(o.join("frames").join(format!("{n}.txt"))).unwrap();
    assert!(frame("03-legend").contains("Esc Esc palette"));
    assert!(frame("07-palette").contains("Command palette"));
    assert!(frame("11-crash").contains("Recover unsaved changes"));
    assert_eq!(std::fs::read_dir(o.join("frames")).unwrap().count(), 13);
    let have = |t: &str| {
        Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {t}"))
            .output()
            .unwrap()
            .status
            .success()
    };
    if have("agg") {
        assert!(o.join("demo.gif").exists());
        if have("ffmpeg") {
            assert!(o.join("demo.mp4").exists());
            let pngs = std::fs::read_dir(o)
                .unwrap()
                .filter(|e| {
                    e.as_ref()
                        .unwrap()
                        .path()
                        .extension()
                        .is_some_and(|x| x == "png")
                })
                .count();
            assert!(pngs >= 6, "only {pngs} PNGs");
        }
    }
}
