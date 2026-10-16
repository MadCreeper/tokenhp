//! Desktop process behavior shared by the GUI and its headless SSH helper.
use std::{
    ffi::OsStr,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

/// Drain concurrently: Windows pipes are small enough that SSH debug output
/// or a server banner can otherwise block authentication before it completes.
pub struct StderrCapture {
    bytes: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl StderrCapture {
    pub fn start(child: &mut std::process::Child) -> Self {
        let pipe = child.stderr.take().expect("SSH stderr must be piped");
        let bytes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = bytes.clone();
        let worker = std::thread::spawn(move || read_stderr_tail(pipe, output));
        Self {
            bytes,
            worker: Some(worker),
        }
    }
    /// Call after the child exits. Bound the wait: a proxy/helper descendant
    /// could have inherited stderr and outlived SSH itself.
    pub fn finish(&mut self) -> String {
        for _ in 0..20 {
            if self.worker.as_ref().is_some_and(|w| w.is_finished()) {
                let _ = self.worker.take().unwrap().join();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        String::from_utf8_lossy(&self.bytes.lock().unwrap())
            .trim()
            .to_string()
    }
}
fn read_stderr_tail(
    mut reader: impl std::io::Read,
    bytes: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
) {
    let mut chunk = [0; 1024];
    while let Ok(n) = reader.read(&mut chunk) {
        if n == 0 {
            break;
        }
        let mut out = bytes.lock().unwrap();
        out.extend_from_slice(&chunk[..n]);
        let excess = out.len().saturating_sub(8192);
        out.drain(..excess);
    }
}

pub const ASKPASS_MODE: &str = "HPBAR_SSH_ASKPASS";
pub const ASKPASS_PASSWORD: &str = "HPBAR_TUNNEL_PW";

/// Some Windows OpenSSH versions cannot spawn a Unicode askpass filename.
/// Prefer its DOS-compatible spelling; if short names are unavailable, keep a
/// private ASCII-path executable copy alive for the lifetime of the tunnel.
pub struct AskpassHelper {
    pub path: std::path::PathBuf,
    #[cfg(windows)]
    directory: Option<std::path::PathBuf>,
}
impl AskpassHelper {
    pub fn prepare(path: std::path::PathBuf, needed: bool) -> Result<Self, String> {
        #[cfg(windows)]
        {
            if needed {
                let compatible = windows_short_path(&path).unwrap_or_else(|| path.clone());
                let name = compatible.as_os_str().to_string_lossy();
                if !name.is_ascii() || name.contains(' ') {
                    return Self::private_copy(&path);
                }
                return Ok(Self {
                    path: compatible,
                    directory: None,
                });
            }
            Ok(Self {
                path,
                directory: None,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = needed;
            Ok(Self { path })
        }
    }
    #[cfg(windows)]
    fn private_copy(source: &Path) -> Result<Self, String> {
        use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr::null_mut};
        #[repr(C)]
        struct SecurityAttributes {
            length: u32,
            descriptor: *mut c_void,
            inherit: i32,
        }
        #[link(name = "advapi32")]
        extern "system" {
            fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
                s: *const u16,
                revision: u32,
                out: *mut *mut c_void,
                size: *mut u32,
            ) -> i32;
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn CreateDirectoryW(path: *const u16, security: *const SecurityAttributes) -> i32;
            fn LocalFree(ptr: *mut c_void) -> *mut c_void;
        }
        let root = std::env::var_os("SystemRoot").ok_or("Windows directory is unavailable.")?;
        let directory = std::path::PathBuf::from(root)
            .join("Temp")
            .join(format!("hpbar-askpass-{}", uuid::Uuid::new_v4()));
        if !directory.as_os_str().to_string_lossy().is_ascii() {
            return Err(
                "OpenSSH needs an ASCII-compatible helper path on this Windows installation."
                    .into(),
            );
        }
        // Protected DACL: owner and SYSTEM only, inherited by the executable.
        // Do not inherit a shared Windows Temp directory's writable ACL.
        let sddl: Vec<u16> = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(format!(
                "Could not secure SSH helper: {}",
                std::io::Error::last_os_error()
            ));
        }
        let attributes = SecurityAttributes {
            length: std::mem::size_of::<SecurityAttributes>() as u32,
            descriptor,
            inherit: 0,
        };
        let wide: Vec<u16> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
        let created = unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) };
        let error = std::io::Error::last_os_error();
        unsafe {
            LocalFree(descriptor);
        }
        if created == 0 {
            return Err(format!("Could not prepare SSH helper: {error}"));
        }
        let helper = Self {
            path: directory.join("askpass.exe"),
            directory: Some(directory),
        };
        std::fs::copy(source, &helper.path)
            .map_err(|e| format!("Could not copy SSH helper: {e}"))?;
        Ok(helper)
    }
}
#[cfg(windows)]
impl Drop for AskpassHelper {
    fn drop(&mut self) {
        if let Some(directory) = &self.directory {
            let _ = std::fs::remove_dir_all(directory);
        }
    }
}

#[cfg(windows)]
fn windows_short_path(path: &Path) -> Option<std::path::PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    #[link(name = "kernel32")]
    extern "system" {
        fn GetShortPathNameW(long: *const u16, short: *mut u16, size: u32) -> u32;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut out = vec![0; 32768];
    let len =
        unsafe { GetShortPathNameW(wide.as_ptr(), out.as_mut_ptr(), out.len() as u32) } as usize;
    (len > 0 && len < out.len()).then(|| std::ffi::OsString::from_wide(&out[..len]).into())
}

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
        #[cfg(windows)]
        let compatible = windows_short_path(helper).unwrap_or_else(|| helper.to_path_buf());
        #[cfg(windows)]
        let helper = compatible.as_path();
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
    #[cfg(windows)]
    #[test]
    fn windows_helper_fallback_is_private_and_removed_after_use() {
        let directory;
        {
            let helper = AskpassHelper::private_copy(&std::env::current_exe().unwrap()).unwrap();
            assert!(helper.path.is_file());
            assert!(helper.path.as_os_str().to_string_lossy().is_ascii());
            directory = helper.directory.clone().unwrap();
        }
        assert!(!directory.exists());
    }
    #[test]
    fn noisy_stderr_is_drained_and_keeps_a_bounded_diagnostic_tail() {
        let bytes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut data = vec![b'x'; 100_000];
        data.extend_from_slice(b"permission denied");
        read_stderr_tail(std::io::Cursor::new(data), bytes.clone());
        let out = bytes.lock().unwrap();
        assert_eq!(out.len(), 8192);
        assert!(out.ends_with(b"permission denied"));
    }
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
