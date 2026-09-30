//! Workstation credential display/delivery policy, matching the Infisical fork.

use crate::error::{IfpError, Result};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::FromRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

#[derive(Default)]
struct Policy {
    reveal: bool,
    destination: Option<Mutex<File>>,
}

static POLICY: OnceLock<Policy> = OnceLock::new();

pub fn prepare(
    reveal: bool,
    acknowledge: bool,
    path: Option<&Path>,
    fd: Option<i32>,
    issues_credential: bool,
) -> Result<()> {
    if reveal != acknowledge {
        return Err(IfpError::ExposureAcknowledgementRequired);
    }
    let delivery = path.is_some() || fd.is_some();
    if (path.is_some() && fd.is_some()) || (reveal && delivery) {
        return Err(IfpError::Usage(
            "Choose private file/descriptor delivery or console reveal, not both".into(),
        ));
    }
    if !issues_credential && (delivery || reveal) {
        return Err(IfpError::Usage(
            "Disclosure/delivery flags apply only to credential-issuing commands".into(),
        ));
    }
    if issues_credential && !delivery && !reveal {
        return Err(IfpError::SecretDeliveryRequired);
    }
    let destination = if let Some(path) = path {
        Some(OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
            .map_err(|_| IfpError::Config("Cannot create private credential file; choose a new path in an existing directory".into()))?)
    } else if let Some(fd) = fd {
        Some(private_descriptor(fd)?)
    } else {
        None
    };
    POLICY
        .set(Policy {
            reveal,
            destination: destination.map(Mutex::new),
        })
        .map_err(|_| IfpError::Config("Credential output policy already initialized".into()))
}

fn private_descriptor(fd: i32) -> Result<File> {
    let invalid = || {
        IfpError::Config("Credential output requires a writable private pipe or owner-private file descriptor >= 3".into())
    };
    if fd < 3 {
        return Err(invalid());
    }
    // Duplicate ownership and check writability before an API mutation can occur.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || flags & libc::O_ACCMODE == libc::O_RDONLY {
        return Err(invalid());
    }
    let copied = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if copied < 0 {
        return Err(invalid());
    }
    let file = unsafe { File::from_raw_fd(copied) };
    let metadata = file.metadata().map_err(|_| invalid())?;
    let kind = metadata.mode() & libc::S_IFMT;
    if kind != libc::S_IFIFO && (kind != libc::S_IFREG || metadata.mode() & 0o077 != 0) {
        return Err(invalid());
    }
    for public in [1, 2] {
        let mut info = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(public, info.as_mut_ptr()) } == 0 {
            let info = unsafe { info.assume_init() };
            if metadata.dev() == info.st_dev && metadata.ino() == info.st_ino {
                return Err(invalid());
            }
        }
    }
    Ok(file)
}

pub fn contains_credential(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields
            .iter()
            .any(|(key, value)| key == "clientSecret" || contains_credential(value)),
        Value::Array(values) => values.iter().any(contains_credential),
        _ => false,
    }
}

fn mask(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if key == "clientSecret" {
                    *value = Value::String("******".into());
                } else {
                    mask(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                mask(value);
            }
        }
        _ => (),
    }
}

pub fn emit(mut value: Value) {
    let policy = POLICY.get_or_init(Policy::default);
    if !policy.reveal && policy.destination.is_none() {
        mask(&mut value);
    }
    let result = if let Some(destination) = &policy.destination {
        destination.lock().map_err(|_| ()).and_then(|mut file| {
            serde_json::to_writer(&mut *file, &value).map_err(|_| ())?;
            file.write_all(b"\n").map_err(|_| ())
        })
    } else {
        println!("{value}");
        Ok(())
    };
    if result.is_err() {
        eprintln!(
            "{}",
            serde_json::json!({
                "error": "secret_output_failed", "exitCode": 1,
                "message": "Credential issuance may have succeeded but delivery failed. Inspect the resource before retrying.",
                "resourceId": value.get("id"),
            })
        );
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_nested_credentials_without_changing_metadata() {
        let mut value = serde_json::json!({"id":"i1", "credentials":[{"clientId":"public", "clientSecret":"synthetic-private"}]});
        assert!(contains_credential(&value));
        mask(&mut value);
        assert_eq!(value["credentials"][0]["clientSecret"], "******");
        assert_eq!(value["credentials"][0]["clientId"], "public");
        assert!(!value.to_string().contains("synthetic-private"));
    }

    #[test]
    fn refuses_public_and_read_only_descriptors() {
        for fd in [0, 1, 2, -1] {
            assert!(private_descriptor(fd).is_err());
        }
    }
}
