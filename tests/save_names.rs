//! Temporary names against files already there. In its own test binary, with
//! one test, so this process's first temporary names are known.

use std::fs;

use kyte::fileio::save_atomic;

#[test]
fn a_save_never_takes_a_name_already_in_use() {
    let d = tempfile::tempdir().unwrap();
    let pid = std::process::id();

    // The first temporary name is the file being saved, the next is in use.
    let target = d.path().join(format!(".kyte-tmp.{pid}.0"));
    let taken = d.path().join(format!(".kyte-tmp.{pid}.1"));
    fs::write(&target, "precious original").unwrap();
    fs::write(&taken, "someone else's").unwrap();
    save_atomic(&target, "saved").unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "saved");
    assert_eq!(fs::read_to_string(&taken).unwrap(), "someone else's");
    assert_eq!(fs::read_dir(d.path()).unwrap().count(), 2, "no leftovers");

    // The same for the backup of a file that's rewritten in place. That save
    // takes the next temporary name, 3, and then a backup name.
    let d = tempfile::tempdir().unwrap();
    let target = d.path().join(format!(".kyte-bak.{pid}.4"));
    let taken = d.path().join(format!(".kyte-bak.{pid}.5"));
    let link = d.path().join("link");
    fs::write(&target, "precious original").unwrap();
    fs::write(&taken, "someone else's").unwrap();
    fs::hard_link(&target, &link).unwrap();
    save_atomic(&target, "saved").unwrap();
    assert_eq!(fs::read_to_string(&link).unwrap(), "saved");
    assert_eq!(fs::read_to_string(&taken).unwrap(), "someone else's");
    assert_eq!(fs::read_dir(d.path()).unwrap().count(), 3, "no leftovers");
}
