use super::*;

#[test]
fn relative_path_inside_cwd_resolves() {
    let cwd = Path::new("/work/project");
    let got = sandboxed(cwd, Path::new("src/main.rs")).unwrap();
    assert_eq!(got, PathBuf::from("/work/project/src/main.rs"));
}

#[test]
fn dot_dot_inside_cwd_resolves() {
    let cwd = Path::new("/work/project");
    let got = sandboxed(cwd, Path::new("src/../README.md")).unwrap();
    assert_eq!(got, PathBuf::from("/work/project/README.md"));
}

#[test]
fn absolute_path_inside_cwd_resolves() {
    let cwd = Path::new("/work/project");
    let got = sandboxed(cwd, Path::new("/work/project/lib.rs")).unwrap();
    assert_eq!(got, PathBuf::from("/work/project/lib.rs"));
}

#[test]
fn relative_dot_dot_escape_is_rejected() {
    let cwd = Path::new("/work/project");
    let err = sandboxed(cwd, Path::new("../secret")).unwrap_err();
    assert!(matches!(err, Error::Tool(_)));
}

#[test]
fn absolute_path_outside_cwd_is_rejected() {
    let cwd = Path::new("/work/project");
    let err = sandboxed(cwd, Path::new("/etc/passwd")).unwrap_err();
    assert!(matches!(err, Error::Tool(_)));
}

#[test]
fn sibling_with_shared_prefix_is_rejected() {
    // Guards the lexical-prefix bug: `/work/project` vs `/work/project-evil`.
    let cwd = Path::new("/work/project");
    let err = sandboxed(cwd, Path::new("/work/project-evil/x")).unwrap_err();
    assert!(matches!(err, Error::Tool(_)));
}
