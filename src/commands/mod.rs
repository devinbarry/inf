pub mod identity;
pub mod project;

use crate::error::{IfpError, Result};
use crate::output::Format;
use colored::Colorize;
use std::io::{IsTerminal, Write};

/// Gate a destructive action. `--yes` always passes.
///
/// Otherwise the rule depends on who is calling. In the default JSON mode the
/// caller is an agent or a script, so there is no prompt at all: prompting
/// would either hang the pipeline or read EOF as consent, and both are worse
/// than a deterministic refusal that names the flag. `--human` is the only mode
/// that prompts, and only when stdin is really a terminal.
///
/// The prompt goes to stderr. Written to stdout it disappeared into the
/// redirect whenever output was captured, leaving the process blocked on a
/// question the user never saw.
pub fn confirm(format: Format, assume_yes: bool, prompt: &str) -> Result<()> {
    if assume_yes {
        return Ok(());
    }

    if !format.is_human() || !std::io::stdin().is_terminal() {
        return Err(IfpError::Config(format!(
            "{prompt}\nRefusing to continue without confirmation. Pass --yes to proceed."
        )));
    }

    eprint!("{prompt} {} ", "[y/N]".dimmed());
    std::io::stderr()
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
        assert!(confirm(Format::Json, true, "delete everything?").is_ok());
        assert!(confirm(Format::Human, true, "delete everything?").is_ok());
    }

    #[test]
    fn non_interactive_without_yes_is_refused() {
        // Tests do not run under a TTY, so this exercises the guard directly.
        let err = confirm(Format::Human, false, "delete project 'billing'?").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("--yes"));
        assert!(msg.contains("billing"));
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn json_mode_never_prompts() {
        // An agent holding a TTY must still get a deterministic refusal rather
        // than a blocked read.
        let err = confirm(Format::Json, false, "delete project 'billing'?").unwrap_err();
        assert!(err.to_string().contains("--yes"));
        assert_eq!(err.exit_code(), 2);
    }
}
