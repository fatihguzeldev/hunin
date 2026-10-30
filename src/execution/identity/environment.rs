//! Effective inherited environment identity before collector/SDK injection.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(in crate::execution) fn environment(
    overrides: &BTreeMap<String, String>,
) -> anyhow::Result<Value> {
    for (key, value) in overrides {
        if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
            anyhow::bail!(
                "environment names must be nonempty without '=' or NUL; values cannot contain NUL"
            );
        }
    }
    let normalize = |key: &std::ffi::OsStr| {
        if cfg!(windows) {
            std::ffi::OsString::from(key.to_string_lossy().to_uppercase())
        } else {
            key.to_owned()
        }
    };
    let mut effective = std::env::vars_os()
        .map(|(k, v)| (normalize(&k), v))
        .collect::<BTreeMap<_, _>>();
    for (key, value) in overrides {
        effective.insert(normalize(std::ffi::OsStr::new(key)), value.into());
    }
    let ignored = [
        "HUNIN_RUN_ID",
        "HUNIN_RECORD_DIR",
        "PWD",
        "OLDPWD",
        "SHLVL",
        "_",
        "TERM",
        "TERMCOLOR",
        "COLORTERM",
        "TERM_PROGRAM",
        "TERM_PROGRAM_VERSION",
        "TERM_SESSION_ID",
        "ITERM_SESSION_ID",
        "WT_SESSION",
        "WT_PROFILE_ID",
        "VSCODE_INJECTION",
        "VSCODE_IPC_HOOK_CLI",
    ];
    let mut hash = Sha256::new();
    let mut count = 0;
    for (key, value) in effective {
        let text = key.to_string_lossy();
        let explicitly_set = overrides.keys().any(|name| {
            if cfg!(windows) {
                text.eq_ignore_ascii_case(name)
            } else {
                text == name.as_str()
            }
        });
        let generated = text == "HUNIN_RUN_ID" || text == "HUNIN_RECORD_DIR";
        if generated
            || (!explicitly_set
                && ignored.iter().any(|ignored| {
                    if cfg!(windows) {
                        text.eq_ignore_ascii_case(ignored)
                    } else {
                        text == *ignored
                    }
                }))
        {
            continue;
        }
        for item in [&key, &value] {
            #[cfg(unix)]
            let bytes = {
                use std::os::unix::ffi::OsStrExt;
                item.as_os_str().as_bytes().to_vec()
            };
            #[cfg(windows)]
            let bytes = {
                use std::os::windows::ffi::OsStrExt;
                item.encode_wide()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>()
            };
            hash.update((bytes.len() as u64).to_le_bytes());
            hash.update(bytes);
        }
        count += 1;
    }
    Ok(
        json!({"fingerprint":format!("{:x}",hash.finalize()),"overrides":overrides,"effective_variable_count":count,"excluded_variables":ignored,"identity_scope":"effective inherited environment plus explicit overrides before collector and SDK injection; inherited values are hashed, not stored"}),
    )
}
