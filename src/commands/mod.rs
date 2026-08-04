pub mod identity;
pub mod project;

use crate::error::{IfpError, Result};
use colored::Colorize;
use std::io::{IsTerminal, Write};

/// Gate a destructive action. `--yes` always passes. Without it we prompt, but
/// only when there is a human to answer: in a pipe or a CI job an unanswered
/// prompt would either hang or read EOF as consent, so a non-interactive run
/// without `--yes` is refused outright.
pub fn confirm(assume_yes: bool, prompt: &str) -> Result<()> {
    if assume_yes {
        return Ok(());
    }

    if !std::io::stdin().is_terminal() {
        return Err(IfpError::Config(format!(
            "{prompt}\nRefusing to continue without confirmation. Pass --yes to proceed \
             non-interactively."
        )));
    }

    print!("{prompt} {} ", "[y/N]".dimmed());
    std::io::stdout()
        .flush()
        .map_err(|e| IfpError::Config(e.to_string()))?;

    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|e| IfpError::Config(e.to_string()))?;

    if matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(IfpError::Aborted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assume_yes_bypasses_every_check() {
        assert!(confirm(true, "delete everything?").is_ok());
    }

    #[test]
    fn non_interactive_without_yes_is_refused() {
        // Tests do not run under a TTY, so this exercises the guard directly.
        let err = confirm(false, "delete project 'billing'?").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--yes"));
        assert!(msg.contains("billing"));
        assert_eq!(err.exit_code(), 2);
    }
}
