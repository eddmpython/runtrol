//! Proof that a person was at this machine, for this decision, just now.
//!
//! The security posture requires dangerous capabilities to be enabled by a physical action at the
//! PC. That sentence needs a type, or it is a convention somebody eventually routes around.
//!
//! [`PcPresence`] is that type. Three properties make it worth something:
//!
//! - **Unforgeable.** The struct has a private field and no public constructor, so no other crate can
//!   name one into existence. The only route is [`PresenceChallenge::answer`], which generates the
//!   word itself and compares it itself, so the caller that displays the prompt never learns enough
//!   to fake the result.
//! - **Bound to one decision.** A witness carries the request it answered. Typing the word to add a
//!   workspace root does not also authorize turning off a permission prompt.
//! - **Fresh.** A witness expires. Presence means somebody is at the machine now, and a witness that
//!   could be stashed and spent later would only prove somebody was there once.
//!
//! # The honest limit
//!
//! None of this constrains the code that displays the prompt. That code must render the word, so it
//! could in principle answer its own challenge. The type system cannot fix that; what fixes it is
//! that the prompt lives in one file on the local plane, and the crate that will handle remote frames
//! does not depend on this module, which the dependency gate asserts.

use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};

use runtrol_provider::{AbsPath, WallMs};

use crate::error::SecurityError;
use crate::id::DeviceId;
use crate::scope::{DeviceScope, LocalScope};

/// How long a challenge stands open before it is a denial.
///
/// Long enough to read the prompt and type a word, short enough that walking away from the machine
/// closes the window rather than leaving it open.
pub const CHALLENGE_WINDOW_MS: u64 = 60_000;

/// How old a witness may be when it is spent.
///
/// Shorter than the challenge window on purpose: answering is a human action that deserves time,
/// while spending is the same call path continuing, which takes milliseconds. A gap larger than this
/// means the witness travelled somewhere, and a witness that travels is a witness that can be
/// replayed.
pub const WITNESS_LIFETIME_MS: u64 = 10_000;

/// Number of words in a challenge phrase.
///
/// One word is guessable by an operator who is not reading. Several words cannot be typed by
/// accident, which is the actual threat here: not an attacker guessing, but a person clicking through
/// a prompt they did not read.
const CHALLENGE_WORDS: usize = 3;

/// The word list a challenge phrase is drawn from.
///
/// Short, unambiguous, unrelated to each other, and never homophones, because the operator reads
/// these off a screen and types them. Deliberately boring: a memorable phrase is one somebody starts
/// typing before reading the rest of the prompt.
const WORDS: &[&str] = &[
    "anchor",
    "basalt",
    "cinder",
    "dovetail",
    "ember",
    "fathom",
    "granite",
    "harbor",
    "indigo",
    "juniper",
    "kelp",
    "lantern",
    "marrow",
    "nimbus",
    "oakum",
    "pallet",
    "quarry",
    "rivet",
    "sextant",
    "tundra",
    "umber",
    "vellum",
    "walnut",
    "yarrow",
    "zephyr",
    "bramble",
    "cobalt",
    "driftwood",
    "elmwood",
    "flint",
];

/// What the operator is being asked to approve.
///
/// A challenge renders this, and the resulting witness carries it, so the thing shown and the thing
/// spent are the same thing. That is the mechanical form of "consent to an unnamed action is not
/// consent".
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum GrantRequest {
    /// Give one device a reviewed set of scopes.
    ///
    /// A set rather than one scope, because the operator reads a prompt and types a phrase once for
    /// the whole set. Splitting it into one typing per scope would not be safer, only slower, and a
    /// prompt somebody has to clear six times is a prompt somebody stops reading.
    DeviceScopes {
        /// Which device.
        device: DeviceId,
        /// Exactly which scopes. Anything not listed here is not authorized by the witness.
        scopes: Vec<DeviceScope>,
    },
    /// Do one thing now that can never be delegated to a device.
    Local(LocalScope),
    /// Add a directory tree to the places work may happen.
    AddWorkspaceRoot {
        /// The proposed root, already canonical and already past the deny list.
        ///
        /// Checked before the operator is asked, so the prompt never offers to approve something
        /// that would be refused afterwards.
        path: AbsPath,
    },
}

impl fmt::Display for GrantRequest {
    /// One line, naming exactly what is being approved.
    ///
    /// This is what the operator reads. It lists every scope rather than counting them, because "3
    /// permissions" is not something a person can consent to.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceScopes { device, scopes } => {
                write!(f, "give device {device} these permissions: ")?;
                for (index, scope) in scopes.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{scope}")?;
                }
                Ok(())
            }
            Self::Local(scope) => write!(f, "do one thing now, at this machine: {scope}"),
            Self::AddWorkspaceRoot { path } => {
                write!(f, "allow work inside {path}")
            }
        }
    }
}

/// A handle meaning "this process owns a surface the operator is physically at".
///
/// "Console" covers both surfaces runtrol has: the terminal it was launched from, and the window it
/// draws. What matters is not which one, but that it belongs to this process and that a remote
/// request handler does not have it.
///
/// Not `Clone` and not `Send`. Cloning would make it two, and sending it would let it arrive
/// somewhere that has no operator in front of it.
pub struct LocalConsole {
    /// Private, so no other crate can construct this by naming its fields.
    sealed: core::marker::PhantomData<*const ()>,
}

/// Whether [`LocalConsole::claim`] has already handed out the one console.
static CONSOLE_CLAIMED: AtomicBool = AtomicBool::new(false);

impl LocalConsole {
    /// Take the one local surface this process has.
    ///
    /// Succeeds while nobody else holds it and returns `None` otherwise. That exclusivity is the whole
    /// guarantee, and it is worth more than it looks: the daemon claims the console while starting its
    /// local plane and holds the handle for the process lifetime, so by the time any request handler
    /// runs there is nothing left to claim. Code holding a console therefore cannot be code serving a
    /// remote request.
    ///
    /// Deliberately not a check for a terminal. runtrol also runs behind a window, where `isatty`
    /// answers no while an operator is very much present.
    #[must_use]
    pub fn claim() -> Option<Self> {
        if CONSOLE_CLAIMED.swap(true, Ordering::SeqCst) {
            return None;
        }
        Some(Self {
            sealed: core::marker::PhantomData,
        })
    }
}

impl Drop for LocalConsole {
    /// Hand the console back.
    ///
    /// Ordinary RAII, and it is what makes the exclusivity real rather than a one-way latch: holding
    /// the handle is what holds the console, so a reader can see the lifetime of the guarantee in the
    /// lifetime of the value. In a running daemon this never fires, because the local plane keeps its
    /// handle until the process ends.
    fn drop(&mut self) {
        CONSOLE_CLAIMED.store(false, Ordering::SeqCst);
    }
}

/// A phrase the operator must type, and the request it approves.
///
/// Consumed by [`Self::answer`], so a challenge is single use. A wrong answer ends it, and the next
/// attempt is a new challenge with a new phrase.
pub struct PresenceChallenge {
    /// Private. The caller renders it through [`Self::prompt`] but cannot run the comparison.
    phrase: String,
    /// After this, any answer is a denial.
    expires_at: WallMs,
    /// What answering this authorizes, and nothing else.
    request: GrantRequest,
}

impl PresenceChallenge {
    /// Open a challenge on a surface the operator is at.
    ///
    /// Takes the console by reference and reads nothing from it. It is a proof obligation, not a
    /// parameter: a caller that cannot produce one cannot open a challenge, and only the local plane
    /// can produce one.
    /// # Errors
    ///
    /// [`SecurityError::ChallengeUnavailable`] when the operating system will not supply randomness for
    /// the phrase. No challenge opens, rather than one opening with a guessable phrase.
    pub fn issue(_console: &LocalConsole, request: GrantRequest) -> Result<Self, SecurityError> {
        Ok(Self {
            phrase: phrase()?,
            expires_at: WallMs::now().plus_millis(CHALLENGE_WINDOW_MS),
            request,
        })
    }

    /// What to show the operator: the action, then the phrase to type.
    ///
    /// Includes the request rather than leaving the caller to describe it, so the words the operator
    /// reads and the request the witness carries cannot drift apart.
    #[must_use]
    pub fn prompt(&self) -> String {
        format!(
            "runtrol wants to {}.\nIf that is what you want, type: {}",
            self.request, self.phrase
        )
    }

    /// What this challenge would authorize.
    #[must_use]
    pub const fn request(&self) -> &GrantRequest {
        &self.request
    }

    /// Check what was typed, consuming the challenge either way.
    ///
    /// Whitespace around the answer is ignored, because it comes from a keyboard. Case is not, and
    /// neither is a partial match: the phrase is displayed exactly as it must be typed.
    ///
    /// # Errors
    ///
    /// [`SecurityError::PresenceTimeout`] past the window, [`SecurityError::PresenceDenied`] for the
    /// wrong phrase.
    pub fn answer(self, typed: &str) -> Result<PcPresence, SecurityError> {
        let now = WallMs::now();
        if now > self.expires_at {
            return Err(SecurityError::PresenceTimeout {
                waited_ms: CHALLENGE_WINDOW_MS,
            });
        }
        if typed.trim() != self.phrase {
            return Err(SecurityError::PresenceDenied);
        }
        Ok(PcPresence {
            granted_at: now,
            request: self.request,
        })
    }
}

/// Proof that a person typed a displayed phrase at this machine, for one named request.
///
/// Cannot be constructed outside this module. Both fields are private, which is already enough to make
/// `PcPresence { .. }` unwritable elsewhere, and there is no public constructor. The only route is
/// [`PresenceChallenge::answer`].
///
/// Not `Copy` and not `Clone`. A witness that could be duplicated could be handed to two grants, and
/// the point of binding it to a request is that it authorizes one thing.
pub struct PcPresence {
    /// When the phrase was typed.
    granted_at: WallMs,
    /// The request the operator read and approved.
    request: GrantRequest,
}

impl PcPresence {
    /// What the operator approved.
    #[must_use]
    pub const fn request(&self) -> &GrantRequest {
        &self.request
    }

    /// Confirm this witness is fresh and answers `attempted`.
    ///
    /// Called by the ledger before authority changes hands. Both halves matter: freshness stops a
    /// replay, and the request match stops a witness earned for one decision being spent on another.
    ///
    /// # Errors
    ///
    /// [`SecurityError::WitnessExpired`] past [`WITNESS_LIFETIME_MS`],
    /// [`SecurityError::WitnessMismatch`] when the witness answers a different request.
    pub fn check(&self, attempted: &GrantRequest) -> Result<(), SecurityError> {
        let age = self
            .granted_at
            .millis_until(WallMs::now())
            .unwrap_or(u64::MAX);
        if age > WITNESS_LIFETIME_MS {
            // A backwards clock lands here too, via the saturating age above. Treating an
            // unmeasurable age as expired is the only safe direction: the alternative accepts a
            // witness whose age nobody knows.
            return Err(SecurityError::WitnessExpired {
                age_ms: age,
                limit_ms: WITNESS_LIFETIME_MS,
            });
        }
        if &self.request != attempted {
            return Err(SecurityError::WitnessMismatch {
                approved: self.request.to_string(),
                attempted: attempted.to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
impl PcPresence {
    /// Build a witness without running a challenge.
    ///
    /// Test only. It exists so tests of the *ledger* can exercise grant logic without serialising on
    /// the console, which is a process-wide singleton by design. The real path through a challenge is
    /// covered by this module's own tests, and `#[cfg(test)]` means no shipped code can reach this.
    pub(crate) fn for_tests(request: GrantRequest) -> Self {
        Self {
            granted_at: WallMs::now(),
            request,
        }
    }
}

impl fmt::Debug for PcPresence {
    /// Never prints the phrase, because there is no phrase to print by this point, and never
    /// suggests the witness is transferable.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PcPresence(for {})", self.request)
    }
}

/// Draw a challenge phrase from the operating system's random source.
///
/// # Errors
///
/// [`SecurityError::ChallengeUnavailable`] when the OS refuses to supply randomness.
///
/// Fallible rather than falling back. A phrase a caller could predict is a challenge that proves
/// nothing, so refusing to open the prompt is the only honest answer: the operator sees an error instead
/// of a permission granted on a formality.
///
/// # Why not a UUID
///
/// This crate already generates UUIDv7 identifiers, so drawing bytes from one looked free. Measured on
/// uuid 1.24: `now_v7` is deliberately **monotonic**, holding the high random bits fixed within a
/// millisecond and incrementing only the low ones. Six consecutive calls produced tails starting `cd 15`
/// every time, so the first two words of every phrase were identical. The test below caught it.
/// `getrandom` is the crate uuid itself calls, so naming it directly costs almost nothing and removes a
/// wrong assumption.
///
/// # What the phrase does and does not protect
///
/// It stops a prompt being cleared without being read, and it stops an unrelated code path guessing an
/// answer. It does not protect against the code that renders the prompt, which necessarily sees the
/// phrase. That limit is named in the module documentation and is closed by the dependency graph.
fn phrase() -> Result<String, SecurityError> {
    // Enough bytes to draw every word without a second syscall in the common case. Duplicates are
    // redrawn, so the loop asks again if a run is unlucky.
    let mut entropy = [0_u8; 16];
    let mut words: Vec<&str> = Vec::with_capacity(CHALLENGE_WORDS);

    while words.len() < CHALLENGE_WORDS {
        getrandom::fill(&mut entropy).map_err(|error| SecurityError::ChallengeUnavailable {
            detail: error.to_string(),
        })?;
        for byte in &entropy {
            if words.len() == CHALLENGE_WORDS {
                break;
            }
            let index = usize::from(*byte) % WORDS.len();
            match WORDS.get(index) {
                // Distinct words only. A repeat reads like a typo and invites a mistyped answer.
                Some(word) if !words.contains(word) => words.push(word),
                // Either the word was already drawn, or the index missed, which cannot happen because
                // it is taken modulo the slice length. Both are answered by reading the next byte.
                Some(_) | None => {}
            }
        }
    }
    Ok(words.join("-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_request() -> GrantRequest {
        GrantRequest::Local(LocalScope::ConfigWrite)
    }

    fn another_request() -> GrantRequest {
        GrantRequest::Local(LocalScope::ModeDangerous)
    }

    /// Runs `body` with the process's one console, then releases it.
    ///
    /// The console is deliberately once-per-process, so tests have to hand it back. Written as a
    /// helper rather than repeated, because forgetting the release makes an unrelated test fail.
    fn with_console<T>(body: impl FnOnce(&LocalConsole) -> T) -> T {
        let _serialised = console_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let console = LocalConsole::claim().expect("the console is free while this lock is held");
        body(&console)
    }

    /// Serialises the tests that claim the console.
    ///
    /// There is exactly one console per process, which is the invariant under test, so tests that take
    /// it cannot run beside each other. A poisoned lock is recovered rather than propagated: poison
    /// means an earlier test panicked, that test already reported its own failure, and turning it into
    /// a cascade of unrelated failures would bury the real one.
    #[expect(
        clippy::disallowed_types,
        reason = "the workspace ban on std Mutex is about holding one across an await. this is \
                  synchronous test-only code, with no async context to deadlock"
    )]
    fn console_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[test]
    fn the_console_is_handed_out_once() {
        // The daemon claims it during local startup, so nothing serving a remote request can hold
        // one. That is the entire guarantee, and this is the test of it.
        let _serialised = console_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let first = LocalConsole::claim().expect("first claim succeeds");
        assert!(
            LocalConsole::claim().is_none(),
            "a second console would let a request handler open a challenge"
        );
        drop(first);
        let again = LocalConsole::claim().expect("a released console can be reclaimed");
        assert!(LocalConsole::claim().is_none(), "still exclusive");
        drop(again);
    }

    #[test]
    fn the_right_phrase_yields_a_witness() {
        with_console(|console| {
            let challenge =
                PresenceChallenge::issue(console, a_request()).expect("randomness is available");
            let phrase = challenge.phrase.clone();
            let witness = challenge
                .answer(&phrase)
                .expect("correct phrase is accepted");
            assert_eq!(witness.request(), &a_request());
        });
    }

    #[test]
    fn the_wrong_phrase_is_a_denial() {
        with_console(|console| {
            let challenge =
                PresenceChallenge::issue(console, a_request()).expect("randomness is available");
            assert!(matches!(
                challenge.answer("wrong"),
                Err(SecurityError::PresenceDenied)
            ));
        });
    }

    #[test]
    fn surrounding_whitespace_is_forgiven_and_case_is_not() {
        with_console(|console| {
            let challenge =
                PresenceChallenge::issue(console, a_request()).expect("randomness is available");
            let phrase = challenge.phrase.clone();
            assert!(challenge.answer(&format!("  {phrase}\n")).is_ok());

            let challenge =
                PresenceChallenge::issue(console, a_request()).expect("randomness is available");
            let shouted = challenge.phrase.to_uppercase();
            assert!(matches!(
                challenge.answer(&shouted),
                Err(SecurityError::PresenceDenied)
            ));
        });
    }

    #[test]
    fn an_expired_challenge_refuses_even_the_right_phrase() {
        with_console(|console| {
            let mut challenge =
                PresenceChallenge::issue(console, a_request()).expect("randomness is available");
            let phrase = challenge.phrase.clone();
            challenge.expires_at = WallMs::from_millis(1);
            assert!(matches!(
                challenge.answer(&phrase),
                Err(SecurityError::PresenceTimeout { .. })
            ));
        });
    }

    #[test]
    fn a_witness_is_refused_for_a_request_it_did_not_answer() {
        // This is the property that stops one typing from authorizing everything: the operator
        // approved config.write, and mode.dangerous is a different decision.
        with_console(|console| {
            let challenge =
                PresenceChallenge::issue(console, a_request()).expect("randomness is available");
            let phrase = challenge.phrase.clone();
            let witness = challenge.answer(&phrase).expect("correct phrase");
            assert!(witness.check(&a_request()).is_ok());
            assert!(matches!(
                witness.check(&another_request()),
                Err(SecurityError::WitnessMismatch { .. })
            ));
        });
    }

    #[test]
    fn a_stale_witness_cannot_be_spent() {
        with_console(|console| {
            let challenge =
                PresenceChallenge::issue(console, a_request()).expect("randomness is available");
            let phrase = challenge.phrase.clone();
            let mut witness = challenge.answer(&phrase).expect("correct phrase");
            witness.granted_at = WallMs::from_millis(1);
            assert!(matches!(
                witness.check(&a_request()),
                Err(SecurityError::WitnessExpired { .. })
            ));
        });
    }

    #[test]
    fn the_prompt_names_the_action_and_the_phrase() {
        // An operator who cannot see what they are approving is not consenting to it.
        with_console(|console| {
            let device = DeviceId::now();
            let request = GrantRequest::DeviceScopes {
                device,
                scopes: vec![DeviceScope::SessionDelete, DeviceScope::SessionInputWrite],
            };
            let challenge =
                PresenceChallenge::issue(console, request).expect("randomness is available");
            let prompt = challenge.prompt();
            assert!(prompt.contains(&device.to_string()), "names the device");
            assert!(prompt.contains("session.delete"), "names every scope");
            assert!(prompt.contains("session.input.write"), "names every scope");
            assert!(prompt.contains(&challenge.phrase), "shows what to type");
        });
    }

    #[test]
    fn phrases_are_several_distinct_words() {
        // Several words cannot be typed by accident. A repeated word reads like a typo.
        let phrase = phrase().expect("randomness is available");
        let words: Vec<&str> = phrase.split('-').collect();
        assert_eq!(words.len(), CHALLENGE_WORDS);
        let mut unique = words.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), words.len(), "repeated word in {phrase}");
        for word in words {
            assert!(WORDS.contains(&word), "{word} is not from the list");
        }
    }

    #[test]
    fn no_position_in_a_phrase_is_stuck() {
        // Two earlier generators failed here, and both failed the same way: a position stopped varying.
        // Seeding from the clock froze every position within a millisecond. Drawing from a UUIDv7 froze
        // the first two, because uuid's v7 holds its high random bits steady inside one millisecond and
        // increments only the low ones. Entropy now comes from the operating system directly.
        //
        // Asserted per position rather than as "no two phrases are equal". With 30 words and three
        // slots there are 24_360 phrases, so 64 draws collide about eight percent of the time by
        // birthday alone; that assertion failed two runs in twenty and a flaky gate is worse than no
        // gate. A frozen position, which is the actual defect class, shows up here with no flake at all.
        const DRAWS: usize = 64;
        const MIN_DISTINCT_PER_POSITION: usize = 8;

        let phrases: Vec<String> = (0..DRAWS)
            .map(|_| phrase().expect("randomness is available"))
            .collect();

        for position in 0..CHALLENGE_WORDS {
            let mut seen: Vec<&str> = phrases
                .iter()
                .filter_map(|phrase| phrase.split('-').nth(position))
                .collect();
            seen.sort_unstable();
            seen.dedup();
            assert!(
                seen.len() >= MIN_DISTINCT_PER_POSITION,
                "word {position} took only {} distinct values across {DRAWS} draws: {seen:?}",
                seen.len()
            );
        }
    }

    #[test]
    fn debug_never_leaks_a_phrase() {
        with_console(|console| {
            let challenge =
                PresenceChallenge::issue(console, a_request()).expect("randomness is available");
            let phrase = challenge.phrase.clone();
            let witness = challenge.answer(&phrase).expect("correct phrase");
            let printed = format!("{witness:?}");
            assert!(
                !printed.contains(&phrase),
                "a log line must not carry the phrase"
            );
            assert!(
                printed.contains("config.write"),
                "but it must say what it authorizes"
            );
        });
    }
}
