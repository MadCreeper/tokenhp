use hpbar_lib::platform::{self, ASKPASS_MODE, ASKPASS_PASSWORD};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("HPBar spaces & 测试 {}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn helper(&self) -> PathBuf {
        let p = self.0.join(if cfg!(windows) {
            "HPBar helper.exe"
        } else {
            "HPBar helper"
        });
        std::fs::copy(env!("CARGO_BIN_EXE_hpbar"), &p).unwrap();
        p
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn actual_executable_helper_preserves_password_without_starting_gui() {
    let scratch = Scratch::new();
    let helper = scratch.helper();
    let password = "  !%&^ $\\\"' 密码🔑 trailing ";
    let out = platform::background_command(&helper)
        .env(ASKPASS_MODE, "1")
        .env(ASKPASS_PASSWORD, password)
        .arg("Password:")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, [password.as_bytes(), b"\n"].concat());
    assert!(out.stderr.is_empty());
    let out = platform::background_command(&helper)
        .env(ASKPASS_MODE, "1")
        .env_remove(ASKPASS_PASSWORD)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
}

fn isolated_ssh(
    helper: &Path,
    known_hosts: &Path,
    port: u16,
    forward: &str,
    password: &str,
) -> Command {
    let base =
        platform::ssh_command("127.0.0.1", "hpbar-smoke", port, forward, password, helper).unwrap();
    let config = known_hosts.with_extension("config");
    std::fs::write(&config, "").unwrap();
    // -o values are parsed as SSH configuration after argv parsing; quote the
    // filename inside that value too, so spaces don't become multiple paths.
    let known_hosts = known_hosts
        .to_string_lossy()
        .replace('\\', "/")
        .replace('"', "\\\"");
    let mut cmd = platform::background_command(base.get_program());
    cmd.arg("-v")
        .arg("-F")
        .arg(config)
        .arg("-o")
        .arg(format!("UserKnownHostsFile=\"{known_hosts}\""))
        .arg("-o")
        .arg(format!("GlobalKnownHostsFile=\"{known_hosts}\""))
        .args(base.get_args());
    for (name, value) in base.get_envs() {
        if let Some(value) = value {
            cmd.env(name, value);
        }
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    cmd
}

#[test]
#[ignore = "run with scripts/test-ssh-platform.py (local ephemeral SSH server)"]
fn real_openssh_password_tunnel_works_without_remote_shell() {
    let ssh_port: u16 = std::env::var("HPBAR_TEST_SSH_PORT")
        .unwrap()
        .parse()
        .unwrap();
    let password = std::env::var("HPBAR_TEST_SSH_PASSWORD").unwrap();
    let scratch = Scratch::new();
    let helper = scratch.helper();
    let local = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let forward = format!("127.0.0.1:{local}:127.0.0.1:5432");
    let mut child = Running(
        isolated_ssh(
            &helper,
            &scratch.0.join("known_hosts"),
            ssh_port,
            &forward,
            &password,
        )
        .spawn()
        .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut stream = loop {
        if let Ok(s) = TcpStream::connect(("127.0.0.1", local)) {
            break s;
        }
        if let Some(status) = child.0.try_wait().unwrap() {
            let mut stderr = String::new();
            child
                .0
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut stderr)
                .unwrap();
            panic!("SSH exited before forwarding opened ({status}): {stderr}");
        }
        assert!(Instant::now() < deadline, "SSH tunnel startup timed out");
        std::thread::sleep(Duration::from_millis(50));
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(b"hpbar-smoke").unwrap();
    let mut response = [0; 11];
    stream.read_exact(&mut response).unwrap();
    assert_eq!(&response, b"hpbar-smoke");
    drop(stream);
    drop(child);
    let mut bad = Running(
        isolated_ssh(
            &helper,
            &scratch.0.join("known_hosts"),
            ssh_port,
            &forward,
            "wrong password",
        )
        .spawn()
        .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = bad.0.try_wait().unwrap() {
            assert!(!status.success());
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Wrong password did not fail promptly"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
