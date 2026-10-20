//! Subprocess support shared with the BDD steps.

use std::path::Path;
use std::process::Command;

pub fn botspy_command() -> Command {
    // Our cross CI targets use these Cargo runner variables. In cross 0.2.5,
    // aarch64's value is "/qemu-runner aarch64"; nested Command invocations
    // do not inherit Cargo's automatic runner wrapping.
    let runner = if cfg!(all(
        target_os = "linux",
        target_env = "musl",
        any(target_arch = "aarch64", target_arch = "x86_64")
    )) {
        std::env::var(format!(
            "CARGO_TARGET_{}_UNKNOWN_LINUX_MUSL_RUNNER",
            std::env::consts::ARCH.to_ascii_uppercase()
        ))
        .ok()
    } else {
        None
    };
    command_with_runner(Path::new(env!("CARGO_BIN_EXE_botspy")), runner.as_deref())
}

fn command_with_runner(binary: &Path, runner: Option<&str>) -> Command {
    match runner {
        Some(runner) => {
            // Cargo's string runner form is whitespace-separated, not a shell
            // command. Keep the executable path and CLI arguments separate.
            let mut args = runner.split_whitespace();
            let mut command = Command::new(args.next().expect("nonempty Cargo target runner"));
            command.args(args).arg(binary);
            command
        }
        None => Command::new(binary),
    }
}

#[test]
fn native_command_preserves_the_executable_path() {
    let binary = Path::new("fixture directory/botspy");
    let command = command_with_runner(binary, None);
    assert_eq!(command.get_program(), binary.as_os_str());
    assert_eq!(command.get_args().count(), 0);
}

#[test]
fn configured_runner_failure_is_not_hidden_by_native_execution() {
    let fixture = tempfile::tempdir().unwrap();
    let missing_runner = fixture.path().join("missing-runner");
    let result = command_with_runner(
        Path::new(env!("CARGO_BIN_EXE_botspy")),
        Some(missing_runner.to_str().unwrap()),
    )
    .arg("--version")
    .output();
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
}

#[cfg(unix)]
#[test]
fn runner_arguments_environment_and_binary_path_survive_the_subprocess() {
    let fixture = tempfile::tempdir().unwrap();
    let binary = fixture.path().join("synthetic executable with spaces");
    std::fs::write(
        &binary,
        "printf '%s\\n' \"$1\" \"$BOTSPY_RUNNER_FIXTURE\"\n",
    )
    .unwrap();
    let literal_argument = "path with spaces; $(not-a-command)";
    let output = command_with_runner(&binary, Some("/bin/sh -e"))
        .arg(literal_argument)
        .env("BOTSPY_RUNNER_FIXTURE", "synthetic environment")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{literal_argument}\nsynthetic environment\n")
    );
}

#[test]
fn botspy_subprocess_runs_on_the_current_target() {
    let output = botspy_command().arg("--version").output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .starts_with("botspy "));
}
