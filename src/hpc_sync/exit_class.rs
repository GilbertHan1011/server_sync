#[derive(Debug, Clone)]
pub struct ExitClass {
    pub name: &'static str,
    pub retryable: bool,
}

pub fn classify_exit(exit_code: i32, stderr: &str) -> ExitClass {
    match exit_code {
        0 => ExitClass {
            name: "success",
            retryable: false,
        },
        24 => ExitClass {
            name: "source_changed",
            retryable: true,
        },
        5 | 10 | 12 | 14 | 30 | 35 => ExitClass {
            name: "transient_transport",
            retryable: true,
        },
        23 => ExitClass {
            name: "partial_data",
            retryable: true,
        },
        25 => ExitClass {
            name: "safety_rejection",
            retryable: false,
        },
        255 if is_authentication_error(stderr) => ExitClass {
            name: "authentication",
            retryable: false,
        },
        255 => ExitClass {
            name: "transient_transport",
            retryable: true,
        },
        _ => ExitClass {
            name: "failed",
            retryable: false,
        },
    }
}

fn is_authentication_error(stderr: &str) -> bool {
    let lowered = stderr.to_ascii_lowercase();
    [
        "host key verification failed",
        "permission denied",
        "no matching host key",
        "private key",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
}
