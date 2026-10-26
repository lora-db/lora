use lora_database::SyncMode;

use super::env::{resolve, EnvInputs};
use super::errors::ConfigError;
use super::{ConfigOutcome, ServerConfig};

fn args(xs: &[&str]) -> Vec<String> {
    std::iter::once("lora-server")
        .chain(xs.iter().copied())
        .map(String::from)
        .collect()
}

fn run_cfg(out: ConfigOutcome) -> ServerConfig {
    match out {
        ConfigOutcome::Run(c) => c,
        other => panic!("expected Run, got {other:?}"),
    }
}

#[test]
fn defaults_when_nothing_set() {
    let cfg = run_cfg(resolve(args(&[]), EnvInputs::default()).unwrap());
    assert_eq!(cfg, ServerConfig::default());
}

#[test]
fn env_vars_apply_without_cli() {
    let cfg = run_cfg(
        resolve(
            args(&[]),
            EnvInputs {
                host: Some("0.0.0.0".into()),
                port: Some("9000".into()),
                ..EnvInputs::default()
            },
        )
        .unwrap(),
    );
    assert_eq!(cfg.host, "0.0.0.0");
    assert_eq!(cfg.port, 9000);
}

#[test]
fn cli_flags_override_env() {
    let cfg = run_cfg(
        resolve(
            args(&["--host", "10.0.0.1", "--port", "8080"]),
            EnvInputs {
                host: Some("0.0.0.0".into()),
                port: Some("9000".into()),
                ..EnvInputs::default()
            },
        )
        .unwrap(),
    );
    assert_eq!(cfg.host, "10.0.0.1");
    assert_eq!(cfg.port, 8080);
}

#[test]
fn cli_equals_form_works() {
    let cfg = run_cfg(resolve(args(&["--host=::1", "--port=7000"]), EnvInputs::default()).unwrap());
    assert_eq!(cfg.host, "::1");
    assert_eq!(cfg.port, 7000);
}

#[test]
fn snapshot_path_from_cli() {
    let cfg = run_cfg(
        resolve(
            args(&["--snapshot-path", "/tmp/snap.bin"]),
            EnvInputs::default(),
        )
        .unwrap(),
    );
    assert_eq!(
        cfg.snapshot_path,
        Some(std::path::PathBuf::from("/tmp/snap.bin"))
    );
}

#[test]
fn snapshot_path_from_env() {
    let cfg = run_cfg(
        resolve(
            args(&[]),
            EnvInputs {
                snapshot_path: Some("/var/lora/snap.bin".into()),
                ..EnvInputs::default()
            },
        )
        .unwrap(),
    );
    assert_eq!(
        cfg.snapshot_path,
        Some(std::path::PathBuf::from("/var/lora/snap.bin"))
    );
}

#[test]
fn cli_snapshot_path_overrides_env() {
    let cfg = run_cfg(
        resolve(
            args(&["--snapshot-path", "/cli/snap.bin"]),
            EnvInputs {
                snapshot_path: Some("/env/snap.bin".into()),
                ..EnvInputs::default()
            },
        )
        .unwrap(),
    );
    assert_eq!(
        cfg.snapshot_path,
        Some(std::path::PathBuf::from("/cli/snap.bin"))
    );
}

#[test]
fn wal_dir_from_cli_and_env() {
    let cfg = run_cfg(resolve(args(&["--wal-dir", "/tmp/wal"]), EnvInputs::default()).unwrap());
    assert_eq!(cfg.wal_dir, Some(std::path::PathBuf::from("/tmp/wal")));

    let cfg = run_cfg(
        resolve(
            args(&[]),
            EnvInputs {
                wal_dir: Some("/env/wal".into()),
                ..EnvInputs::default()
            },
        )
        .unwrap(),
    );
    assert_eq!(cfg.wal_dir, Some(std::path::PathBuf::from("/env/wal")));

    // CLI overrides env.
    let cfg = run_cfg(
        resolve(
            args(&["--wal-dir=/cli/wal"]),
            EnvInputs {
                wal_dir: Some("/env/wal".into()),
                ..EnvInputs::default()
            },
        )
        .unwrap(),
    );
    assert_eq!(cfg.wal_dir, Some(std::path::PathBuf::from("/cli/wal")));
}

#[test]
fn wal_sync_mode_parses_known_strings() {
    for raw in ["group-sync", "GROUP_SYNC", "groupsync"] {
        let cfg = run_cfg(resolve(args(&["--wal-sync-mode", raw]), EnvInputs::default()).unwrap());
        assert_eq!(
            cfg.wal_sync_mode,
            SyncMode::GroupSync { interval_ms: 50 },
            "raw={raw}"
        );
    }
}

#[test]
fn invalid_wal_sync_mode_rejected() {
    let err = resolve(args(&["--wal-sync-mode", "yolo"]), EnvInputs::default()).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidSyncMode(_)));
}

#[test]
fn help_flag_returns_help_outcome() {
    match resolve(args(&["--help"]), EnvInputs::default()).unwrap() {
        ConfigOutcome::Help(s) => assert!(s.contains("USAGE")),
        other => panic!("expected Help, got {other:?}"),
    }
}

#[test]
fn version_flag_returns_version_outcome() {
    match resolve(args(&["--version"]), EnvInputs::default()).unwrap() {
        ConfigOutcome::Version(s) => assert!(s.starts_with("lora-server ")),
        other => panic!("expected Version, got {other:?}"),
    }
}

#[test]
fn invalid_port_is_rejected() {
    let err = resolve(args(&["--port", "notanumber"]), EnvInputs::default()).unwrap_err();
    match err {
        ConfigError::InvalidPort { value, .. } => assert_eq!(value, "notanumber"),
        other => panic!("expected InvalidPort, got {other:?}"),
    }
}

#[test]
fn port_out_of_range_is_rejected() {
    let err = resolve(args(&["--port", "70000"]), EnvInputs::default()).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidPort { .. }));
}

#[test]
fn missing_value_is_rejected() {
    let err = resolve(args(&["--host"]), EnvInputs::default()).unwrap_err();
    assert_eq!(err, ConfigError::MissingValue("--host"));
}

#[test]
fn unknown_flag_is_rejected() {
    let err = resolve(args(&["--nope"]), EnvInputs::default()).unwrap_err();
    assert_eq!(err, ConfigError::UnknownArg("--nope".into()));
}

#[test]
fn positional_is_rejected() {
    let err = resolve(args(&["something"]), EnvInputs::default()).unwrap_err();
    assert_eq!(err, ConfigError::UnexpectedPositional("something".into()));
}

#[test]
fn ipv4_bind_addr_format() {
    let cfg = ServerConfig {
        host: "127.0.0.1".into(),
        port: 3000,
        ..ServerConfig::default()
    };
    assert_eq!(cfg.bind_addr(), "127.0.0.1:3000");
}

#[test]
fn ipv6_bind_addr_is_bracketed() {
    let cfg = ServerConfig {
        host: "::1".into(),
        port: 3000,
        ..ServerConfig::default()
    };
    assert_eq!(cfg.bind_addr(), "[::1]:3000");
}

#[test]
fn empty_host_rejected() {
    let err = resolve(args(&["--host", "   "]), EnvInputs::default()).unwrap_err();
    assert_eq!(err, ConfigError::EmptyValue("--host"));
}

#[test]
fn query_limit_flags_parse_from_cli_and_env() {
    let cfg = run_cfg(resolve(args(&[]), EnvInputs::default()).unwrap());
    assert_eq!(cfg.query_timeout_ms, super::DEFAULT_QUERY_TIMEOUT_MS);
    assert_eq!(cfg.max_body_bytes, super::DEFAULT_MAX_BODY_BYTES);
    assert!(cfg.max_concurrent_queries > 0);
    assert_eq!(cfg.admin_path_root, None);

    let cfg = run_cfg(
        resolve(
            args(&[
                "--query-timeout-ms",
                "0",
                "--max-concurrent-queries=3",
                "--admin-path-root",
                "/srv/lora",
            ]),
            EnvInputs {
                query_timeout_ms: Some("5000".into()),
                max_body_bytes: Some("1024".into()),
                ..EnvInputs::default()
            },
        )
        .unwrap(),
    );
    assert_eq!(cfg.query_timeout_ms, 0);
    assert_eq!(cfg.max_concurrent_queries, 3);
    assert_eq!(cfg.max_body_bytes, 1024);
    assert_eq!(
        cfg.admin_path_root,
        Some(std::path::PathBuf::from("/srv/lora"))
    );
}

#[test]
fn invalid_query_limit_rejected() {
    let err = resolve(args(&["--query-timeout-ms", "soon"]), EnvInputs::default()).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::InvalidNumber {
            flag: "--query-timeout-ms",
            ..
        }
    ));
    let err = resolve(args(&["--max-body-bytes", " "]), EnvInputs::default()).unwrap_err();
    assert_eq!(err, ConfigError::EmptyValue("--max-body-bytes"));
}

#[test]
fn result_cap_flags_parse_from_cli_and_env() {
    let cfg = run_cfg(resolve(args(&[]), EnvInputs::default()).unwrap());
    assert_eq!(cfg.max_result_rows, super::DEFAULT_MAX_RESULT_ROWS);
    assert_eq!(cfg.max_response_bytes, super::DEFAULT_MAX_RESPONSE_BYTES);

    let cfg = run_cfg(
        resolve(
            args(&["--max-result-rows", "0", "--max-response-bytes=4096"]),
            EnvInputs {
                max_result_rows: Some("10".into()),
                max_response_bytes: Some("1".into()),
                ..EnvInputs::default()
            },
        )
        .unwrap(),
    );
    assert_eq!(cfg.max_result_rows, 0);
    assert_eq!(cfg.max_response_bytes, 4096);

    let env = EnvInputs {
        max_result_rows: Some("500".into()),
        ..EnvInputs::default()
    };
    let cfg = run_cfg(resolve(args(&[]), env).unwrap());
    assert_eq!(cfg.max_result_rows, 500);

    let err = resolve(args(&["--max-result-rows", "-1"]), EnvInputs::default()).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::InvalidNumber {
            flag: "--max-result-rows",
            ..
        }
    ));
}

#[test]
fn header_read_timeout_parses_from_cli_and_env() {
    let cfg = run_cfg(resolve(args(&[]), EnvInputs::default()).unwrap());
    assert_eq!(
        cfg.header_read_timeout_ms,
        super::DEFAULT_HEADER_READ_TIMEOUT_MS
    );

    let env = EnvInputs {
        header_read_timeout_ms: Some("2500".into()),
        ..EnvInputs::default()
    };
    let cfg = run_cfg(resolve(args(&[]), env.clone()).unwrap());
    assert_eq!(cfg.header_read_timeout_ms, 2500);
    let cfg = run_cfg(resolve(args(&["--header-read-timeout-ms", "0"]), env).unwrap());
    assert_eq!(cfg.header_read_timeout_ms, 0);
}

#[test]
fn wal_truncate_corrupt_tail_is_a_switch() {
    let cfg = run_cfg(resolve(args(&[]), EnvInputs::default()).unwrap());
    assert!(!cfg.wal_truncate_corrupt_tail);

    // Bare, it does not take the next argument as its value.
    let cfg = run_cfg(
        resolve(
            args(&["--wal-truncate-corrupt-tail", "--wal-dir", "/w"]),
            EnvInputs::default(),
        )
        .unwrap(),
    );
    assert!(cfg.wal_truncate_corrupt_tail);
    assert_eq!(cfg.wal_dir, Some(std::path::PathBuf::from("/w")));

    let env = EnvInputs {
        wal_truncate_corrupt_tail: Some("true".into()),
        ..EnvInputs::default()
    };
    assert!(run_cfg(resolve(args(&[]), env.clone()).unwrap()).wal_truncate_corrupt_tail);
    let cfg = run_cfg(resolve(args(&["--wal-truncate-corrupt-tail=false"]), env).unwrap());
    assert!(!cfg.wal_truncate_corrupt_tail);

    let err = resolve(
        args(&["--wal-truncate-corrupt-tail=maybe"]),
        EnvInputs::default(),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ConfigError::InvalidBool {
            flag: "--wal-truncate-corrupt-tail",
            ..
        }
    ));
}
