use std::fs;

use super::*;

#[test]
fn reflinks_when_the_filesystem_supports_it() {
    let base = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => panic!("tempdir: {e}"),
    };
    if !ReflinkBackend.supports(base.path()) {
        return;
    }

    let src = base.path().join("src");
    fs::create_dir_all(src.join("nested")).expect("mkdir");
    fs::write(src.join("nested/f.txt"), "reflinked bytes\n").expect("write");

    let dest = base.path().join("dest");
    ReflinkBackend.copy_dir(&src, &dest).expect("copy_dir");
    assert_eq!(
        fs::read_to_string(dest.join("nested/f.txt")).expect("read"),
        "reflinked bytes\n"
    );

    assert!(matches!(
        ReflinkBackend.copy_dir(&src, &dest),
        Err(crate::Error::DestinationExists)
    ));
}

#[test]
fn reflink_preserves_the_source_mode_exactly() {
    use std::os::unix::fs::PermissionsExt;

    let base = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => panic!("tempdir: {e}"),
    };
    if !ReflinkBackend.supports(base.path()) {
        return;
    }

    let src = base.path().join("src");
    fs::create_dir_all(&src).expect("mkdir");
    fs::write(src.join("script.sh"), "#!/bin/sh\necho hi\n").expect("write");
    fs::set_permissions(src.join("script.sh"), fs::Permissions::from_mode(0o755)).expect("chmod");
    fs::write(src.join("secret.txt"), "shh\n").expect("write");
    fs::set_permissions(src.join("secret.txt"), fs::Permissions::from_mode(0o600)).expect("chmod");

    let dest = base.path().join("dest");
    ReflinkBackend.copy_dir(&src, &dest).expect("copy_dir");

    for (name, want) in [("script.sh", 0o755), ("secret.txt", 0o600)] {
        assert_eq!(
            fs::metadata(dest.join(name))
                .expect("meta")
                .permissions()
                .mode()
                & 0o7777,
            want,
            "{name} must keep its exact mode after FICLONE"
        );
    }
}
