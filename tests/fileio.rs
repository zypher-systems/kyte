//! File IO tests (task T3): AC-03 (library level), AC-05, AC-24.

use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use kyte::fileio::{
    complete_path, disk_state, is_read_only, load, save_atomic, state_dir_from, OpenError,
};
use rustix::fs::{mknodat, setxattr, FileType, Mode, XattrFlags, CWD};

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
    if fs::metadata(d.path()).unwrap().uid() == 0 {
        eprintln!("skipped: root can write to anything");
        return;
    }
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
fn hard_linked_file_stays_one_file_with_no_leftovers() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.txt");
    let link = d.path().join("b.txt");
    fs::write(&p, "old\n").unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
    fs::hard_link(&p, &link).unwrap();
    let inode = fs::metadata(&p).unwrap().ino();
    save_atomic(&p, "new and longer\n").unwrap();
    save_atomic(&p, "short\n").unwrap();
    assert_eq!(fs::read_to_string(&link).unwrap(), "short\n");
    assert_eq!(fs::metadata(&p).unwrap().ino(), inode);
    assert_eq!(
        fs::metadata(&p).unwrap().permissions().mode() & 0o7777,
        0o640
    );
    assert_eq!(names(d.path()), vec!["a.txt", "b.txt"]);
}

#[test]
fn extended_attributes_survive_a_save() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.txt");
    fs::write(&p, "old\n").unwrap();
    if setxattr(&p, "user.kyte", b"kept", XattrFlags::empty()).is_err() {
        eprintln!("skipped: no user xattrs on this file system");
        return;
    }
    save_atomic(&p, "new\n").unwrap();
    let mut value = [0u8; 16];
    let n = rustix::fs::getxattr(&p, "user.kyte", &mut value[..]).unwrap();
    assert_eq!(&value[..n], b"kept");
    assert_eq!(fs::read_to_string(&p).unwrap(), "new\n");
    assert_eq!(names(d.path()), vec!["a.txt"]);
}

#[test]
fn large_extended_attribute_survives_a_save() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.txt");
    fs::write(&p, "old\n").unwrap();
    let big = vec![b'x'; 8000];
    let small = b"y";
    if setxattr(&p, "user.big", &big, XattrFlags::empty()).is_err()
        || setxattr(&p, "user.small", small, XattrFlags::empty()).is_err()
    {
        eprintln!("skipped: no large user xattrs on this file system");
        return;
    }
    save_atomic(&p, "new\n").unwrap();
    let mut value = vec![0u8; 16384];
    let n = rustix::fs::getxattr(&p, "user.big", &mut value[..]).unwrap();
    assert_eq!(&value[..n], &big[..]);
}

#[test]
fn fifos_and_devices_are_refused() {
    let d = tempfile::tempdir().unwrap();
    let fifo = d.path().join("pipe");
    mknodat(CWD, &fifo, FileType::Fifo, Mode::from_raw_mode(0o600), 0).unwrap();
    assert!(matches!(load(&fifo), Err(OpenError::NotRegular)));
    assert!(matches!(
        load(Path::new("/dev/zero")),
        Err(OpenError::NotRegular)
    ));
}

#[test]
fn disk_state_sees_content_not_timestamps() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.txt");
    assert_eq!(disk_state(&p).unwrap(), None);
    fs::write(&p, "abc").unwrap();
    let first = disk_state(&p).unwrap();
    fs::write(&p, "abc").unwrap();
    assert_eq!(disk_state(&p).unwrap(), first);
    fs::write(&p, "abd").unwrap();
    assert_ne!(disk_state(&p).unwrap(), first);
}

#[test]
fn read_only_detection() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.txt");
    assert!(!is_read_only(&p), "a missing file isn't read-only");
    fs::write(&p, "x").unwrap();
    assert!(!is_read_only(&p));
    fs::set_permissions(&p, fs::Permissions::from_mode(0o444)).unwrap();
    if fs::metadata(d.path()).unwrap().uid() == 0 {
        eprintln!("skipped: root can write to anything");
        return;
    }
    assert!(is_read_only(&p));
}

#[test]
fn completion_extends_to_what_the_choices_share() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("notes-one.txt"), "").unwrap();
    fs::write(d.path().join("notes-two.txt"), "").unwrap();
    fs::write(d.path().join(".hidden"), "").unwrap();
    fs::create_dir(d.path().join("src")).unwrap();
    let base = format!("{}/", d.path().display());
    let c = complete_path(&format!("{base}no")).unwrap();
    assert_eq!(c.text, format!("{base}notes-"));
    assert_eq!(c.choices, vec!["notes-one.txt", "notes-two.txt"]);
    let c = complete_path(&format!("{base}notes-o")).unwrap();
    assert_eq!(c.text, format!("{base}notes-one.txt"));
    assert_eq!(c.choices.len(), 1);
    // a directory gets its slash, ready for the next part
    assert_eq!(
        complete_path(&format!("{base}s")).unwrap().text,
        format!("{base}src/")
    );
    // hidden files wait for a dot
    assert_eq!(complete_path(&base).unwrap().choices.len(), 3);
    assert_eq!(
        complete_path(&format!("{base}.")).unwrap().text,
        format!("{base}.hidden")
    );
    assert_eq!(complete_path(&format!("{base}zzz")), None);
    assert_eq!(complete_path("/no/such/dir/x"), None);
}

#[test]
fn completion_keeps_multi_byte_names_whole() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join("世界a"), "").unwrap();
    fs::write(d.path().join("世界b"), "").unwrap();
    let base = format!("{}/", d.path().display());
    let c = complete_path(&format!("{base}世")).unwrap();
    assert_eq!(c.text, format!("{base}世界"));
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
