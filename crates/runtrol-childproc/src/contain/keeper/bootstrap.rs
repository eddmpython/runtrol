//! Private same-image entry, before ordinary Runtime assembly or process containment.

use std::fs::File;
use std::io::BufReader;

use super::{KeeperCompletion, handles, protocol, server};
use crate::SpawnError;
use crate::contain::job::failure;

pub(super) const ARGUMENT: &str = "__runtrol-job-keeper";

/// Run only the inherited-handle keeper personality requested by this exact executable.
///
/// The callback receives positive completion only after the original Runtime and all retained scopes
/// end. It must compare the existing durable ownership rows before recording that fact.
pub fn bootstrap_if_requested(
    words: &[String],
    completed: impl FnOnce(KeeperCompletion) -> Result<(), String>,
) -> Option<Result<(), SpawnError>> {
    if words.first().map(String::as_str) != Some(ARGUMENT) {
        return None;
    }
    Some(run(words, completed))
}

fn run(
    words: &[String],
    completed: impl FnOnce(KeeperCompletion) -> Result<(), String>,
) -> Result<(), SpawnError> {
    let mut fields = words.iter().skip(1).map(String::as_str);
    let parent: usize = protocol::number(fields.next())?;
    let input: usize = protocol::number(fields.next())?;
    let output: usize = protocol::number(fields.next())?;
    if fields.next().is_some() {
        return Err(failure(
            "starting private keeper",
            "extra bootstrap handles",
        ));
    }
    if parent == input || parent == output || input == output {
        return Err(failure(
            "starting private keeper",
            "bootstrap handles must be distinct",
        ));
    }
    let parent = handles::Process::received(parent)?;
    let input = super::job_handle(input)?;
    let output = super::job_handle(output)?;
    let mut input = BufReader::new(File::from(input));
    let (target, limits) = protocol::read_target(&mut input)?;
    let proof = server::run(parent, input, File::from(output), target, limits)?;
    completed(proof).map_err(|detail| failure("recording exact keeper completion", detail))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::{AsHandle as _, AsRawHandle as _};

    #[test]
    fn invalid_or_duplicated_bootstrap_handles_never_transfer_ownership() {
        let invalid = [
            ARGUMENT.to_owned(),
            "0".to_owned(),
            "1".to_owned(),
            "2".to_owned(),
        ];
        assert!(
            run(&invalid, |_| panic!(
                "invalid handles cannot complete a generation"
            ))
            .is_err()
        );
        let parent = handles::current().unwrap();
        let raw = (parent.handle.as_raw_handle() as usize).to_string();
        let duplicated = [ARGUMENT.to_owned(), raw.clone(), raw.clone(), raw];
        assert!(
            run(&duplicated, |_| panic!(
                "duplicated handles cannot complete a generation"
            ))
            .is_err()
        );
        assert_eq!(
            handles::identity(parent.handle.as_handle()).unwrap(),
            parent.identity
        );
    }
}
