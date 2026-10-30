//! File IO tests (task T3): AC-03 (library level), AC-05, AC-24.

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use kyte::fileio::{load, save_atomic, state_dir_from, OpenError};

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn load_nonexistent_is_empty_and_not_existing() {
    let d = tempfile::tempdir().unwrap();
    let l = load(&d.path().join("new.txt")).unwrap();
    assert_eq!(l.text, "");
    assert!(!l.existed);
    assert_eq!(l.mode, None);
}

#[test]
fn ac03_load_directory_is_refused() {
    let d = tempfile::tempdir().unwrap();
    assert!(matches!(load(d.path()), Err(OpenError::IsDirectory)));
}

#[test]
fn ac03_non_utf8_is_refused_and_file_untouched() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("bin.dat");
    let bytes = vec![0x66, 0x6f, 0xff, 0xfe, 0x00, 0x80];
    fs::write(&p, &bytes).unwrap();
    assert!(matches!(load(&p), Err(OpenError::NotUtf8)));
    assert_eq!(fs::read(&p).unwrap(), bytes);
}

#[test]
fn load_keeps_bytes_exactly() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.txt");
    fs::write(&p, "a\r\nb\n\nno-trailing").unwrap();
    let l = load(&p).unwrap();
    assert_eq!(l.text, "a\r\nb\n\nno-trailing");
    assert!(l.existed);
}

#[test]
fn ac05_save_preserves_mode_trailing_newline_and_leaves_no_temp() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("f.txt");
    fs::write(&p, "line one\nline two\n").unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
    let l = load(&p).unwrap();
    assert_eq!(l.mode, Some(0o640));
    save_atomic(&p, "Xline one\nline two\n").unwrap();
    assert_eq!(fs::read_to_string(&p).unwrap(), "Xline one\nline two\n");
    assert_eq!(
        fs::metadata(&p).unwrap().permissions().mode() & 0o7777,
        0o640
    );
    assert_eq!(names(d.path()), vec!["f.txt".to_string()]);
}

#[test]
fn save_creates_new_file() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("new.txt");
    save_atomic(&p, "hello\nworld").unwrap();
    assert_eq!(fs::read_to_string(&p).unwrap(), "hello\nworld");
    assert_eq!(names(d.path()), vec!["new.txt".to_string()]);
}

#[test]
fn ac24_read_only_directory_leaves_original_untouched() {
    let d = tempfile::tempdir().unwrap();
    let sub = d.path().join("ro");
    fs::create_dir(&sub).unwrap();
    let p = sub.join("f.txt");
    fs::write(&p, "original\n").unwrap();
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o555)).unwrap();
    let r = save_atomic(&p, "changed\n");
    let after = fs::read_to_string(&p).unwrap();
    let left = names(&sub);
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(r.is_err(), "save into a read-only directory must fail");
    assert_eq!(after, "original\n");
    assert_eq!(left, vec!["f.txt".to_string()], "no temp file may remain");
}

#[test]
fn ac24_symlink_target_updated_and_link_intact() {
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real.txt");
    let link = d.path().join("link.txt");
    fs::write(&real, "old\n").unwrap();
    symlink(&real, &link).unwrap();
    save_atomic(&link, "new\n").unwrap();
    assert_eq!(fs::read_to_string(&real).unwrap(), "new\n");
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&link).unwrap(), real);
    assert_eq!(
        names(d.path()),
        vec!["link.txt".to_string(), "real.txt".to_string()]
    );
}

#[test]
fn state_dir_precedence() {
    let k = Some(PathBuf::from("/k"));
    let x = Some(PathBuf::from("/x"));
    let h = Some(PathBuf::from("/h"));
    assert_eq!(
        state_dir_from(k.clone(), x.clone(), h.clone()),
        PathBuf::from("/k")
    );
    assert_eq!(
        state_dir_from(None, x.clone(), h.clone()),
        PathBuf::from("/x/kyte/swap")
    );
    assert_eq!(
        state_dir_from(None, None, h.clone()),
        PathBuf::from("/h/.local/state/kyte/swap")
    );
    assert_eq!(
        state_dir_from(Some(PathBuf::new()), None, h),
        PathBuf::from("/h/.local/state/kyte/swap")
    );
}
