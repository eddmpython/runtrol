//! What a driver is asked to do, and what it hands back.
//!
//! # A prompt is carried, never rewritten
//!
//! runtrol relays what the operator wrote. It does not prepend, append, summarize, reformat, or add a system
//! prompt of its own, and there is nowhere in these types to put one. [`ContentBlock::Native`] exists so a
//! surface can send a block shape runtrol has never heard of, which is the same rule pointed outwards: a
//! capability runtrol does not know about is still usable through it.
//!
//! # Why a command enum instead of a method per command
//!
//! Because a surface has to be able to drive a provider feature runtrol has no binding for.
//! [`AgentCommand::Native`] forwards a frame verbatim, and a method-per-command shape has nowhere for that to
//! live. It is the escape hatch that keeps runtrol a pipe rather than a gate, and the reason the phone can use a
//! CLI feature that shipped after this binary did.

use crate::event::{EventBody, Opaque};
use crate::id::{ApprovalId, OptionId, SessionId};
use crate::path::AbsPath;

/// One piece of what the operator is sending.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ContentBlock {
    /// Text, exactly as it was written.
    Text(Box<str>),
    /// A block shape runtrol has no binding for, forwarded whole.
    ///
    /// What lets a surface use a provider feature that arrived after this binary did.
    Native(Opaque),
}

/// What to tell a running session to do.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum AgentCommand {
    /// Send what the operator wrote.
    Prompt(Vec<ContentBlock>),
    /// Stop the turn that is running.
    ///
    /// An interrupt is a request, not an outcome. What ends the turn is still the provider's own word, and a
    /// driver that reported the turn ended because it asked for an interrupt would be inventing a completion.
    Interrupt,
    /// Answer something the provider asked a human.
    Answer {
        /// Which prompt.
        id: ApprovalId,
        /// Which of the options the provider offered.
        option: OptionId,
        /// The digest of the subject the answerer was looking at.
        ///
        /// Echoed back because two prompts can be open at once, and consent to one is not consent to the other.
        subject_digest: [u8; 32],
    },
    /// Forward a frame verbatim.
    ///
    /// Never inspected and never rewritten. The escape hatch that keeps runtrol a pipe: a surface can drive a
    /// feature runtrol has never heard of, and runtrol stays out of the way.
    Native(Opaque),
}

/// Why a session is being opened.
#[derive(Clone, Debug)]
pub struct OpenIntent {
    /// The identifier runtrol minted for this session.
    ///
    /// Offered to a provider that accepts one. Measured on one of them: it comes back unchanged and becomes the
    /// name its own store knows the session by, which is what makes deleting everything runtrol keeps harmless.
    pub session: SessionId,
    /// Where the agent works.
    pub workspace: AbsPath,
    /// Whether this is a new conversation or a continuation.
    pub disposition: Disposition,
    /// The model runtrol is asking for, when the operator chose one.
    ///
    /// `None` means the provider's own settings decide, which is the honest default: runtrol does not have an
    /// opinion about which model somebody wants.
    pub model: Option<Box<str>>,
    /// The permission posture to start at, when the operator chose one.
    pub permission: Option<Box<str>>,
}

/// New, or continuing something that already exists.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Disposition {
    /// A conversation that does not exist yet.
    Fresh,
    /// A conversation the provider already has.
    Resume {
        /// The provider's own name for it.
        ///
        /// The provider's and not runtrol's, because a resume command takes the name the provider knows. For the
        /// provider that accepts runtrol's identifier the two are equal, and nothing here depends on that.
        native: Box<str>,
    },
}

/// One event a driver produced, before the hub numbers it.
///
/// A driver supplies what happened and how far into the provider's own store it corresponds to. It does not
/// supply a position: numbering happens at the one point where a driver's output enters a session, so that a
/// driver turning one provider line into three events never has to think about it and two drivers across a
/// reattach cannot collide.
#[derive(Clone, Debug)]
pub struct Produced {
    /// How far into the provider's own store this corresponds to.
    ///
    /// The unit is the driver's business. The kernel compares it and never interprets it, which is what keeps
    /// provider-specific knowledge out of the kernel.
    pub src_end: u64,
    /// What happened.
    pub body: EventBody,
}

/// How to end a session.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum CloseMode {
    /// Ask, and wait this long for the provider to finish what it was doing.
    Graceful {
        /// How long to wait before stopping it anyway.
        grace_ms: u64,
    },
    /// Stop it now.
    Kill,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_has_nowhere_to_put_something_runtrol_added() {
        // The thin rule, as a shape rather than a promise. A block is either what the operator wrote or a native
        // block forwarded whole, and there is no third case for runtrol's own words.
        let written = "do the thing";
        let blocks = [ContentBlock::Text(written.into())];
        match blocks.first() {
            Some(ContentBlock::Text(text)) => assert_eq!(&**text, written),
            other => panic!("expected the operator's own text, got {other:?}"),
        }
    }

    #[test]
    fn a_surface_can_drive_a_feature_runtrol_has_never_heard_of() {
        // Both directions of the escape hatch: a block shape and a whole command. Without them runtrol is a gate
        // on which provider features are reachable, and a feature that ships after this binary is unreachable
        // until somebody rebuilds it.
        let native = Opaque::owned(r#"{"type":"something_new","payload":{"deep":1}}"#.to_owned());
        let command = AgentCommand::Native(native.clone());
        match command {
            AgentCommand::Native(payload) => assert!(payload.as_str().contains("something_new")),
            other => panic!("expected a forwarded frame, got {other:?}"),
        }
        assert!(matches!(
            ContentBlock::Native(native),
            ContentBlock::Native(_)
        ));
    }

    #[test]
    fn an_answer_carries_the_subject_it_was_given_to() {
        // Two prompts can be open at once, and consent to one is not consent to the other. Without the digest an
        // answer says which prompt and not which subject, and a subject that changed underneath would be
        // answered for.
        let command = AgentCommand::Answer {
            id: ApprovalId::now(),
            option: OptionId(1),
            subject_digest: [7; 32],
        };
        match command {
            AgentCommand::Answer { subject_digest, .. } => assert_eq!(subject_digest, [7; 32]),
            other => panic!("expected an answer, got {other:?}"),
        }
    }

    #[test]
    fn an_interrupt_is_a_request_and_not_an_outcome() {
        // Nothing about this command says the turn ended. What ends a turn is the provider's own word, and a
        // driver that treated its own interrupt as a completion would be inventing one.
        let command = AgentCommand::Interrupt;
        assert!(matches!(command, AgentCommand::Interrupt));
    }

    #[test]
    fn a_driver_supplies_what_happened_and_never_where_it_sits_in_the_stream() {
        // Numbering happens at one point, in the hub. A driver that assigned positions would collide with the
        // next driver across a reattach, and would have to reason about turning one line into three events.
        let produced = Produced {
            src_end: 4_096,
            body: EventBody::Plan(Opaque::none()),
        };
        assert_eq!(produced.src_end, 4_096);
        let printed = format!("{produced:?}");
        assert!(
            !printed.contains("seq"),
            "a produced event has no position to print: {printed}"
        );
    }

    #[test]
    fn resuming_takes_the_name_the_provider_knows() {
        // A resume command is given to the provider, so it takes the provider's own name. For the CLI that
        // accepts runtrol's identifier the two are equal, and nothing here relies on that.
        let intent = Disposition::Resume {
            native: "some-provider-name".into(),
        };
        match intent {
            Disposition::Resume { native } => assert_eq!(&*native, "some-provider-name"),
            other => panic!("expected a resume, got {other:?}"),
        }
    }

    #[test]
    fn no_model_and_no_permission_means_the_providers_own_settings_decide() {
        // The honest default. runtrol does not have an opinion about which model somebody wants, and inventing
        // one would override a choice the operator already made in the CLI's own configuration.
        let intent = OpenIntent {
            session: SessionId::now(),
            workspace: AbsPath::new(if cfg!(windows) { r"C:\work" } else { "/work" })
                .expect("valid"),
            disposition: Disposition::Fresh,
            model: None,
            permission: None,
        };
        assert!(intent.model.is_none());
        assert!(intent.permission.is_none());
    }
}
