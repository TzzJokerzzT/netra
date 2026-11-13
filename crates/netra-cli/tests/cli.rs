//! CLI acceptance tests. Every test runs in an isolated data directory so the
//! developer's real sessions are never touched.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{fs, path::Path, process::Command};

use assert_cmd::prelude::*;
use predicates::prelude::*;

struct Sandbox {
    directory: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            directory: tempfile::tempdir().unwrap(),
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::cargo_bin("netra").unwrap();
        command.env("XDG_DATA_HOME", self.directory.path());
        command.env("NETRA_LOG", "error");
        // Never let the startup check or `netra update` reach the public API:
        // the loopback override points at a closed port, so every attempt fails
        // fast and silently instead of touching the network.
        command.env("NETRA_UPDATE_API_BASE", loopback_override());
        command
    }

    fn data_dir(&self) -> std::path::PathBuf {
        self.directory.path().join("netra")
    }

    fn database(&self) -> std::path::PathBuf {
        self.data_dir().join("netra.db")
    }
}

fn session_prefix(stdout: &str) -> String {
    let line = stdout
        .lines()
        .find(|line| line.starts_with("session "))
        .expect("capture prints the session line");
    line.split_whitespace()
        .nth(1)
        .expect("session id is printed")
        .to_owned()
}

#[test]
fn help_documents_only_implemented_commands() {
    let sandbox = Sandbox::new();
    sandbox
        .command()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("capture"))
        .stdout(predicate::str::contains("sessions"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("import").not())
        .stdout(predicate::str::contains("replay").not());
}

#[test]
fn config_reports_loopback_and_redaction_defaults() {
    let sandbox = Sandbox::new();
    sandbox
        .command()
        .arg("config")
        .assert()
        .success()
        .stdout(predicate::str::contains("127.0.0.1:8080"))
        .stdout(predicate::str::contains("tls interception disabled"))
        .stdout(predicate::str::contains("redaction"))
        .stdout(predicate::str::contains("netra.db"));
    assert!(sandbox.data_dir().exists());
}

#[cfg(unix)]
#[test]
fn database_is_created_with_private_permissions() {
    use std::os::unix::fs::PermissionsExt as _;
    let sandbox = Sandbox::new();
    sandbox
        .command()
        .arg("sessions")
        .arg("list")
        .assert()
        .success();
    let mode = fs::metadata(sandbox.database())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn empty_session_list_is_not_an_error() {
    let sandbox = Sandbox::new();
    sandbox
        .command()
        .args(["sessions", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no sessions yet"));
}

/// A port nothing listens on, so a test can never attach to a developer's
/// running browser.
fn closed_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Loopback API base that no server answers, used to keep tests offline.
fn loopback_override() -> String {
    format!("http://127.0.0.1:{}", closed_port())
}

#[test]
fn version_reports_the_build_it_came_from() {
    let sandbox = Sandbox::new();
    let expected = format!("netra {} (unknown)", env!("CARGO_PKG_VERSION"));
    sandbox
        .command()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(expected.clone()));
    sandbox
        .command()
        .arg("-V")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
    // A local build has no commit to report, and says so instead of guessing.
    assert!(expected.ends_with("(unknown)"));
}

#[test]
fn update_refuses_a_non_loopback_http_override() {
    let sandbox = Sandbox::new();
    sandbox
        .command()
        .env("NETRA_UPDATE_API_BASE", "http://api.example.com:80")
        .args(["update", "--check"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("NETRA_UPDATE_API_BASE"))
        .stderr(predicate::str::contains("loopback"));
}

#[test]
fn update_refuses_check_together_with_force() {
    let sandbox = Sandbox::new();
    sandbox
        .command()
        .args(["update", "--check", "--force"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--check"))
        .stderr(predicate::str::contains("--force"));
}

#[test]
fn update_without_a_reachable_api_fails_cleanly() {
    let sandbox = Sandbox::new();
    sandbox
        .command()
        .args(["update", "--check"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("netra: cannot reach"))
        .stderr(predicate::str::contains("panicked").not())
        .stderr(predicate::str::contains("RUST_BACKTRACE").not());
}

#[test]
fn config_reports_the_update_check_switch() {
    let sandbox = Sandbox::new();
    sandbox
        .command()
        .arg("config")
        .assert()
        .success()
        .stdout(predicate::str::contains("startup check enabled"))
        .stdout(predicate::str::contains("update-check.json"));
    sandbox
        .command()
        .args(["config", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"update\""))
        .stdout(predicate::str::contains("\"check\": true"));
}

/// Seeds the cached release answer the startup notice reads.
fn seed_cache(sandbox: &Sandbox, latest: &str, age: chrono::Duration) -> std::path::PathBuf {
    let data_dir = sandbox.data_dir();
    fs::create_dir_all(&data_dir).unwrap();
    let path = data_dir.join("update-check.json");
    let checked_at = (chrono::Utc::now() - age).to_rfc3339();
    fs::write(
        &path,
        format!(r#"{{"checked_at":"{checked_at}","latest":"{latest}"}}"#),
    )
    .unwrap();
    path
}

#[test]
fn a_newer_cached_release_is_announced_on_stderr() {
    let sandbox = Sandbox::new();
    seed_cache(&sandbox, "0.2.0", chrono::Duration::hours(1));
    sandbox
        .command()
        .args(["sessions", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no sessions yet"))
        .stderr(predicate::str::contains(
            "⚠ netra 0.2.0 disponible — corré `netra update`",
        ));
}

#[test]
fn a_current_cached_release_is_not_announced() {
    let sandbox = Sandbox::new();
    seed_cache(
        &sandbox,
        env!("CARGO_PKG_VERSION"),
        chrono::Duration::hours(1),
    );
    sandbox
        .command()
        .args(["sessions", "list"])
        .assert()
        .success()
        .stderr(predicate::str::contains("disponible").not());
}

#[test]
fn a_disabled_update_check_prints_no_notice_and_keeps_the_cache() {
    let sandbox = Sandbox::new();
    let data_dir = sandbox.data_dir();
    fs::create_dir_all(&data_dir).unwrap();
    fs::write(
        data_dir.join("config.json"),
        r#"{"update":{"check":false}}"#,
    )
    .unwrap();
    let cache = seed_cache(&sandbox, "0.2.0", chrono::Duration::hours(48));
    let before = fs::read_to_string(&cache).unwrap();

    sandbox
        .command()
        .args(["sessions", "list"])
        .assert()
        .success()
        .stderr(predicate::str::contains("disponible").not());
    assert_eq!(
        fs::read_to_string(&cache).unwrap(),
        before,
        "a disabled check never rewrites the cache"
    );
    sandbox
        .command()
        .arg("config")
        .assert()
        .success()
        .stdout(predicate::str::contains("startup check disabled"));
}

#[test]
fn a_disabled_update_check_never_creates_the_cache() {
    let sandbox = Sandbox::new();
    let data_dir = sandbox.data_dir();
    fs::create_dir_all(&data_dir).unwrap();
    fs::write(
        data_dir.join("config.json"),
        r#"{"update":{"check":false}}"#,
    )
    .unwrap();
    sandbox
        .command()
        .args(["sessions", "list"])
        .assert()
        .success();
    assert!(!data_dir.join("update-check.json").exists());
}

#[test]
fn cdp_capture_without_a_browser_fails_clearly_and_leaves_no_session() {
    let sandbox = Sandbox::new();
    let endpoint = format!("127.0.0.1:{}", closed_port());
    sandbox
        .command()
        .args(["capture", "--source", "cdp", "--cdp-endpoint", &endpoint])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("remote-debugging-port"));
    sandbox
        .command()
        .args(["sessions", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no sessions yet"));
}

#[test]
fn ca_commands_refuse_to_invent_material() {
    let sandbox = Sandbox::new();
    sandbox
        .command()
        .args(["ca", "show"])
        .assert()
        .code(4)
        .stderr(predicate::str::contains("netra ca init"));
    sandbox
        .command()
        .args(["ca", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("certificate authority ready"));
    let certificate = sandbox.data_dir().join("ca").join("netra-ca.pem");
    assert!(certificate.exists());
    sandbox.command().args(["ca", "init"]).assert().success();
    let exported = sandbox.directory.path().join("trusted-ca.pem");
    sandbox
        .command()
        .args(["ca", "export", "--output"])
        .arg(&exported)
        .assert()
        .success();
    let pem = fs::read_to_string(&exported).unwrap();
    assert!(pem.contains("BEGIN CERTIFICATE"));
    sandbox
        .command()
        .args(["ca", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("SHA-256"))
        .stdout(predicate::str::contains("disabled"));
}

#[test]
fn proxy_capture_starts_and_is_reported_in_the_config() {
    let sandbox = Sandbox::new();
    sandbox
        .command()
        .arg("config")
        .assert()
        .success()
        .stdout(predicate::str::contains("tls interception disabled"));
}

#[test]
fn interception_without_a_ca_requires_an_explicit_init() {
    let sandbox = Sandbox::new();
    let config = sandbox.directory.path().join("proxy-config.json");
    fs::write(
        &config,
        r#"{"proxy":{"bind_addr":"127.0.0.1","port":18080,"max_connections":4,"intercept_tls":true}}"#,
    )
    .unwrap();
    sandbox
        .command()
        .arg("--config")
        .arg(&config)
        .args(["capture", "--source", "proxy"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("netra ca init"));
}

#[test]
fn demo_capture_persists_exchanges_and_export_produces_har() {
    let sandbox = Sandbox::new();
    let output = sandbox
        .command()
        .args(["capture", "--demo", "--source", "cdp", "--name", "demo run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(stdout.contains("capturing from cdp"), "{stdout}");
    assert!(stdout.contains("persisted 5 exchanges"), "{stdout}");
    assert!(stdout.contains("(1 failed)"), "{stdout}");

    let prefix = session_prefix(&stdout);
    sandbox
        .command()
        .args(["sessions", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains(prefix.clone()))
        .stdout(predicate::str::contains("stopped"))
        .stdout(predicate::str::contains("demo run"));

    let export_path = sandbox.directory.path().join("out.har");
    sandbox
        .command()
        .args(["sessions", "export", &prefix])
        .arg("--output")
        .arg(&export_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("exported 5 exchanges"));
    let har = fs::read_to_string(&export_path).unwrap();
    let document: serde_json::Value = serde_json::from_str(&har).unwrap();
    assert_eq!(document["log"]["version"], "1.2");
    assert_eq!(document["log"]["entries"].as_array().unwrap().len(), 5);
    assert!(
        har.contains("https://api.demo.test/v1/users?page=1"),
        "the exported URL is the captured one"
    );
}

#[test]
fn captured_credentials_are_redacted_before_storage() {
    let sandbox = Sandbox::new();
    let output = sandbox
        .command()
        .args(["capture", "--demo", "--source", "cdp"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    let prefix = session_prefix(&stdout);
    let export_path = sandbox.directory.path().join("redacted.har");
    sandbox
        .command()
        .args(["sessions", "export", &prefix])
        .arg("--output")
        .arg(&export_path)
        .assert()
        .success();
    let har = fs::read_to_string(&export_path).unwrap();
    assert!(!har.contains("demo-token"), "authorization must be masked");
    assert!(!har.contains("demo-secret"), "body secrets must be masked");
    assert!(har.contains("[REDACTED]"));
}

#[test]
fn save_and_delete_require_the_right_arguments() {
    let sandbox = Sandbox::new();
    let output = sandbox
        .command()
        .args(["capture", "--demo", "--source", "cdp"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let prefix = session_prefix(&String::from_utf8(output).unwrap());

    sandbox
        .command()
        .args(["sessions", "save", &prefix])
        .assert()
        .success()
        .stdout(predicate::str::contains("saved"));

    sandbox
        .command()
        .args(["sessions", "delete", &prefix, "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("deleted"));

    sandbox
        .command()
        .args(["sessions", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no sessions yet"));
}

#[test]
fn unknown_session_exits_with_not_found() {
    let sandbox = Sandbox::new();
    sandbox
        .command()
        .args(["sessions", "open", "does-not-exist"])
        .assert()
        .code(4)
        .stderr(predicate::str::contains("no session matches"));
}

#[test]
fn invalid_source_is_a_usage_error() {
    let sandbox = Sandbox::new();
    sandbox
        .command()
        .args(["capture", "--source", "carrier-pigeon"])
        .assert()
        .code(2);
}

#[test]
fn interrupted_session_is_recovered_on_the_next_run() {
    let sandbox = Sandbox::new();
    let output = sandbox
        .command()
        .args(["capture", "--demo", "--source", "cdp"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let prefix = session_prefix(&String::from_utf8(output).unwrap());

    // Simulate a process that died mid-capture by flipping the stored status.
    let connection = rusqlite::Connection::open(sandbox.database()).unwrap();
    connection
        .execute(
            "UPDATE sessions SET status = 'capturing', finished_at = NULL WHERE id LIKE ?1 || '%'",
            [prefix.as_str()],
        )
        .unwrap();
    drop(connection);

    sandbox
        .command()
        .args(["sessions", "list"])
        .assert()
        .success()
        .stderr(predicate::str::contains("recovered 1 interrupted session"));

    let connection = rusqlite::Connection::open(sandbox.database()).unwrap();
    let status: String = connection
        .query_row(
            "SELECT status FROM sessions WHERE id LIKE ?1 || '%'",
            [prefix.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "stopped");
}

#[test]
fn a_live_capture_is_not_recovered_by_another_command() {
    let sandbox = Sandbox::new();
    let mut child = sandbox
        .command()
        .args(["capture", "--demo", "--source", "cdp"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(150));
    sandbox
        .command()
        .args(["sessions", "list", "--json"])
        .assert()
        .success()
        .stderr(predicate::str::contains("recovered").not());
    let connection = rusqlite::Connection::open(sandbox.database()).unwrap();
    let status: String = connection
        .query_row("SELECT status FROM sessions LIMIT 1", [], |row| row.get(0))
        .unwrap();
    drop(connection);
    assert_eq!(status, "capturing", "a live capture must stay capturing");
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
#[test]
fn wal_and_shm_are_private_while_capture_runs() {
    use std::os::unix::fs::PermissionsExt as _;
    let sandbox = Sandbox::new();
    let mut child = sandbox
        .command()
        .args(["capture", "--demo", "--source", "cdp"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    for name in ["netra.db", "netra.db-wal", "netra.db-shm"] {
        let path = sandbox.data_dir().join(name);
        if path.exists() {
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{name} must be private, got {mode:o}");
        }
    }
    let directory = fs::metadata(sandbox.data_dir())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(directory, 0o700);
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
#[test]
fn exported_har_files_are_private() {
    use std::os::unix::fs::PermissionsExt as _;
    let sandbox = Sandbox::new();
    let output = sandbox
        .command()
        .args(["capture", "--demo", "--source", "cdp"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let prefix = session_prefix(&String::from_utf8(output).unwrap());
    let export_path = sandbox.directory.path().join("private.har");
    let _ = fs::remove_file(&export_path);
    sandbox
        .command()
        .args(["sessions", "export", &prefix])
        .arg("--output")
        .arg(&export_path)
        .assert()
        .success();
    let mode = fs::metadata(&export_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "an export holds captured credentials");
}

#[test]
fn configured_data_dir_is_honoured() {
    let sandbox = Sandbox::new();
    let custom = sandbox.directory.path().join("custom-data");
    let config = sandbox.directory.path().join("netra-config.json");
    fs::write(&config, format!(r#"{{"data_dir":"{}"}}"#, custom.display())).unwrap();
    sandbox
        .command()
        .arg("--config")
        .arg(&config)
        .arg("config")
        .assert()
        .success()
        .stdout(predicate::str::contains(custom.display().to_string()));
    sandbox
        .command()
        .arg("--config")
        .arg(&config)
        .args(["sessions", "list"])
        .assert()
        .success();
    assert!(
        custom.join("netra.db").exists(),
        "the configured data dir must actually be used"
    );
}

#[test]
fn adding_a_redaction_rule_keeps_the_default_carriers() {
    let sandbox = Sandbox::new();
    let config = sandbox.directory.path().join("netra-config.json");
    fs::write(&config, r#"{"redaction":{"header_names":["x-my-token"]}}"#).unwrap();
    let output = sandbox
        .command()
        .arg("--config")
        .arg(&config)
        .args(["capture", "--demo", "--source", "cdp"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let prefix = session_prefix(&String::from_utf8(output).unwrap());
    let export_path = sandbox.directory.path().join("rules.har");
    sandbox
        .command()
        .arg("--config")
        .arg(&config)
        .args(["sessions", "export", &prefix])
        .arg("--output")
        .arg(&export_path)
        .assert()
        .success();
    let har = fs::read_to_string(&export_path).unwrap();
    assert!(
        !har.contains("demo-token") && !har.contains("session=demo"),
        "listing one extra rule must not drop Authorization/Cookie"
    );
}

#[test]
fn export_to_an_unwritable_path_fails_without_panicking() {
    let sandbox = Sandbox::new();
    let output = sandbox
        .command()
        .args(["capture", "--demo", "--source", "cdp"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let prefix = session_prefix(&String::from_utf8(output).unwrap());
    let bogus = Path::new("/proc/netra-cannot-write.har");
    sandbox
        .command()
        .args(["sessions", "export", &prefix])
        .arg("--output")
        .arg(bogus)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("cannot"));
}
