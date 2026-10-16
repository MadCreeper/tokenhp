//! Desktop process behavior shared by the GUI and its headless SSH helper.
use std::{
    ffi::OsStr,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

pub const ASKPASS_MODE: &str = "HPBAR_SSH_ASKPASS";
pub const ASKPASS_PASSWORD: &str = "HPBAR_TUNNEL_PW";

/// OpenSSH starts this executable with inherited stdout redirected to a pipe.
/// Run before Tauri starts: no second window, tray, database access, or logging.
pub fn run_ssh_askpass() -> Option<i32> {
    if std::env::var_os(ASKPASS_MODE).as_deref() != Some(OsStr::new("1")) {
        return None;
    }
    let Some(password) = std::env::var(ASKPASS_PASSWORD).ok() else {
        return Some(1);
    };
    if password.contains(['\r', '\n', '\0']) {
        return Some(1);
    }
    let mut out = std::io::stdout().lock();
    Some(
        if out
            .write_all(password.as_bytes())
            .and_then(|_| out.write_all(b"\n"))
            .and_then(|_| out.flush())
            .is_ok()
        {
            0
        } else {
            1
        },
    )
}

/// GUI-subsystem parents otherwise briefly flash a console for each SSH or
/// hostname subprocess on Windows. Unix behavior is unchanged.
pub fn background_command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

fn ssh_program() -> std::ffi::OsString {
    // Prefer native OpenSSH over Git/MSYS ssh, whose path/environment rules
    // differ. Also works when Explorer's PATH predates OpenSSH installation.
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        let native = std::path::PathBuf::from(root).join("System32/OpenSSH/ssh.exe");
        if native.is_file() {
            return native.into_os_string();
        }
    }
    "ssh".into()
}

pub fn ssh_command(
    host: &str,
    user: &str,
    port: u16,
    forward: &str,
    password: &str,
    helper: &Path,
) -> Result<Command, String> {
    if password.contains(['\r', '\n', '\0']) {
        return Err("SSH password must be a single line.".into());
    }
    let mut cmd = background_command(ssh_program());
    cmd.args([
        "-N",
        "-T",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ConnectTimeout=10",
        "-p",
        &port.to_string(),
        "-L",
        forward,
    ]);
    if password.is_empty() {
        cmd.args(["-o", "BatchMode=yes"]);
    } else {
        cmd.env(ASKPASS_MODE, "1")
            .env(ASKPASS_PASSWORD, password)
            .env("SSH_ASKPASS", helper)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("DISPLAY", ":0")
            .args([
                "-o",
                "BatchMode=no",
                "-o",
                "NumberOfPasswordPrompts=1",
                "-o",
                "PreferredAuthentications=password,keyboard-interactive",
            ]);
    }
    cmd.arg("--")
        .arg(format!("{user}@{host}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    Ok(cmd)
}

/// Use the native shell API, not cmd.exe/start: paths/URLs can contain &, %, or
/// spaces and must never be interpreted as shell commands.
#[cfg(windows)]
pub fn open_external(target: &str) -> Result<(), String> {
    use std::{ffi::c_void, ptr::null};
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: *const c_void,
            verb: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> isize;
    }
    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *const c_void, flags: u32) -> i32;
        fn CoUninitialize();
    }
    if target.contains('\0') {
        return Err("Invalid installer path or URL.".into());
    }
    let file: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
    let verb: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    // The caller runs on a blocking worker, not necessarily a COM-initialized
    // GUI thread. Balance successful initialization, including S_FALSE.
    let initialized = unsafe { CoInitializeEx(null(), 0x2 | 0x4) } >= 0;
    let result = unsafe { ShellExecuteW(null(), verb.as_ptr(), file.as_ptr(), null(), null(), 1) };
    if initialized {
        unsafe { CoUninitialize() };
    }
    if result <= 32 {
        Err(format!(
            "Windows could not open the installer or link (error {result})."
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_password_and_key_only_modes_are_distinct() {
        let helper = Path::new("Program Files/HPBar/hpbar.exe");
        let cmd = ssh_command(
            "example",
            "hpbar",
            22,
            "127.0.0.1:54321:127.0.0.1:5432",
            " %!& \"密码",
            helper,
        )
        .unwrap();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"-N".into()));
        assert!(args.contains(&"BatchMode=no".into()));
        assert!(!args.iter().any(|a| a.contains("密码")));
        let env: std::collections::HashMap<_, _> = cmd.get_envs().collect();
        assert_eq!(env[OsStr::new("SSH_ASKPASS")], Some(helper.as_os_str()));
        assert_eq!(
            env[OsStr::new("SSH_ASKPASS_REQUIRE")],
            Some(OsStr::new("force"))
        );
        let cmd = ssh_command("example", "hpbar", 22, "forward", "", helper).unwrap();
        assert!(cmd.get_args().any(|a| a == "BatchMode=yes"));
        assert!(!cmd.get_envs().any(|(k, _)| k == ASKPASS_PASSWORD));
        assert!(ssh_command("example", "hpbar", 22, "forward", "bad\npassword", helper).is_err());
    }
}
