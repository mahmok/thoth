use std::process::Command;

/// Runs `thoth <flag>` and returns its standard output.
fn output_of(flag: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_thoth")).arg(flag).output().expect("thoth should be runnable");
    assert!(output.status.success(), "`thoth {flag}` exited with {}", output.status);
    String::from_utf8(output.stdout).expect("version output is UTF-8")
}

#[test]
fn long_version_flag_reports_the_crate_version() {
    assert_eq!(output_of("--version").trim(), format!("thoth {}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn short_version_flag_reports_the_crate_version() {
    assert_eq!(output_of("-V").trim(), format!("thoth {}", env!("CARGO_PKG_VERSION")));
}
