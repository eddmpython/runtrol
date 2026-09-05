//! Provider identity and options only. An initial body is read separately from bounded stdin.

use super::CourierFailure;
use super::words::{timeout, wrong};

pub(super) const HELP: &str = "runtrol courier spawn PROVIDER [--model MODEL] [--task] [--timeout SECONDS]\n\
With --task, stdin supplies one initial tell. The worker must enable dialogue before consuming it.";

pub(super) struct SpawnCommand {
    pub(super) provider: String,
    pub(super) model: Option<String>,
    pub(super) task: bool,
    pub(super) timeout_ms: u64,
}

pub(super) fn parse(words: &[String]) -> Result<SpawnCommand, CourierFailure> {
    let (provider, words) = words.split_first().ok_or_else(wrong)?;
    if provider.is_empty()
        || provider.starts_with('-')
        || provider.len() > 256
        || provider.chars().any(char::is_control)
    {
        return Err(wrong());
    }
    let mut command = SpawnCommand {
        provider: provider.clone(),
        model: None,
        task: false,
        timeout_ms: runtrol_courier::Limits::INITIAL.default_deadline_millis,
    };
    let mut timed = false;
    let mut words = words.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--task" if !command.task => command.task = true,
            "--model" if command.model.is_none() => {
                let model = words.next().ok_or_else(wrong)?;
                if model.is_empty()
                    || model.len() > 256
                    || model.starts_with('-')
                    || model.chars().any(char::is_control)
                {
                    return Err(wrong());
                }
                command.model = Some(model.clone());
            }
            "--timeout" if !timed => {
                command.timeout_ms = timeout(words.next().ok_or_else(wrong)?)?;
                timed = true;
            }
            _ => return Err(wrong()),
        }
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_has_no_path_or_body_argument_and_rejects_duplicate_options() {
        for words in [
            vec!["provider", "body"],
            vec!["provider", "--task", "body"],
            vec!["provider", "--cwd", "C:/another"],
            vec!["provider", "--task", "--task"],
            vec!["provider", "--model", "one", "--model", "two"],
            vec!["provider", "--timeout", "1", "--timeout", "2"],
            vec!["provider", "--timeout", "0"],
            vec!["provider", "--model"],
        ] {
            assert!(parse(&words.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err());
        }
        let command = parse(
            &[
                "provider",
                "--task",
                "--model",
                "discovered",
                "--timeout",
                "60",
            ]
            .map(str::to_owned),
        )
        .expect("valid");
        assert!(command.task);
        assert_eq!(command.model.as_deref(), Some("discovered"));
        assert_eq!(command.timeout_ms, 60_000);
    }
}
