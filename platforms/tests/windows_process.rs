#![cfg(windows)]

use hunin_platforms::{inspect_process, process::ErrorKind};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
};

#[test]
fn an_exited_process_is_gone_even_while_its_handle_is_retained() {
    if std::env::var_os("HUNIN_RETAINED_PROCESS_CHILD").is_some() {
        std::io::stdin().read_exact(&mut [0]).unwrap();
        std::process::exit(259);
    }
    for kill in [true, false] {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "an_exited_process_is_gone_even_while_its_handle_is_retained",
            ])
            .env("HUNIN_RETAINED_PROCESS_CHILD", "1")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let alive = inspect_process(child.id());
        if kill {
            child.kill().unwrap();
        } else {
            child.stdin.take().unwrap().write_all(b"x").unwrap();
        }
        let status = child.wait().unwrap();
        assert!(alive.is_ok());
        if !kill {
            assert_eq!(status.code(), Some(259));
        }
        assert_eq!(
            inspect_process(child.id()).unwrap_err().kind,
            ErrorKind::ProcessGone
        );
        drop(child);
    }
}
