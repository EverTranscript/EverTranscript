//! The Knob: which Backend runs, and what happens when it fails.
//!
//! **This is the module where a bug sends someone's meeting to a stranger.**
//! Every other failure in this product is recoverable — a lost recording, a
//! missed meeting, a mislabelled speaker. This one is not.
//!
//! Three properties carry that, and each is arranged so that being wrong
//! requires more than a typo.
//!
//! 1. **No preselection** (ADR-0013). [`Choice`] has no `Default`, and the
//!    Knob holds an `Option`. A fresh install cannot generate a Summary
//!    because there is nothing to generate it with, which is the intended
//!    behaviour rather than a gap: every configuration this product runs
//!    traces to an explicit Operator act.
//!
//! 2. **The fallback is one-way, structurally.** [`run`] takes the chosen
//!    Backend and a `local_fallback` factory — and there is no parameter
//!    through which a cloud Backend could be reached as a fallback. The
//!    asymmetry is in the signature, not in a branch: it is not that the
//!    local→cloud case is handled correctly, it is that there is nowhere to
//!    write it. A boolean that happens to be false is a weaker guarantee
//!    than a function that cannot express the wrong thing.
//!
//! 3. **Cancellation never falls back.** An Operator who pressed stop must
//!    not discover that stopping sent their transcript to a provider.

use super::Backend;
use super::BackendError;
use super::BackendIdentity;
use super::Cancel;
use super::Request;

/// What the Operator chose.
///
/// No `Default`, deliberately (ADR-0013): the picker offers Local
/// (Recommended) and Cloud, and Continue stays disabled until one is picked.
/// A type with a default is a type that can be chosen by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Local,
    Cloud {
        /// Which preset or custom endpoint, for the credential lookup and
        /// the indicator.
        provider: String,
    },
}

/// This installation's Summary configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Knob {
    /// None until the Operator has chosen. Not a bug to be defaulted away.
    pub choice: Option<Choice>,
    /// Strict Mode (story 39): never auto-switch; report the failure.
    pub strict: bool,
    /// Whether the one-time cloud warning has been shown and accepted
    /// (story 36). Choosing Cloud without it is refused.
    pub cloud_warning_accepted: bool,
}

impl Knob {
    /// Whether Summary can run at all.
    pub fn is_configured(&self) -> bool {
        self.choice.is_some()
    }

    /// Records the Operator's choice.
    ///
    /// Choosing Cloud before the warning has been accepted is refused rather
    /// than silently allowed — story 36 makes the warning the gate, and a
    /// gate that can be walked around by a Client that forgot to call it is
    /// not a gate.
    pub fn choose(&mut self, choice: Choice) -> Result<(), &'static str> {
        if matches!(choice, Choice::Cloud { .. }) && !self.cloud_warning_accepted {
            return Err(
                "cloud Summary requires the one-time warning to be shown and accepted first",
            );
        }
        self.choice = Some(choice);
        Ok(())
    }
}

/// What actually happened when a Summary was generated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub text: String,
    /// Which Backend produced this — the *active* one, which story 38 wants
    /// visible and which is not always the configured one.
    pub used: BackendIdentity,
    /// Set when the chosen Backend failed and local answered instead. An
    /// Operator who chose Cloud and received local quality is owed this;
    /// a silent fallback is a quality change with no explanation.
    pub fell_back_from: Option<String>,
}

/// Runs a generation, falling back to local if the chosen Backend fails.
///
/// **The signature is the guarantee.** `chosen` may be anything; the
/// fallback is `local_fallback` and nothing else can be passed as one. There
/// is no argument, field, or branch through which a failing local Backend
/// could reach a cloud one — so the local→cloud path is not merely absent,
/// it is unwritable here without changing this function's type.
pub fn run(
    knob: &Knob,
    chosen: &mut dyn Backend,
    local_fallback: Option<&mut dyn Backend>,
    request: &Request,
    cancel: &Cancel,
) -> Result<Outcome, BackendError> {
    let identity = chosen.identity();
    let failure = match chosen.generate(request, cancel) {
        Ok(text) => {
            return Ok(Outcome {
                text,
                used: identity,
                fell_back_from: None,
            });
        }
        Err(error) => error,
    };

    // The Operator stopping is not a failure to route around.
    if !failure.is_worth_falling_back_from() {
        return Err(failure);
    }

    // Strict Mode: tell them, do not switch (story 39).
    if knob.strict {
        return Err(failure);
    }

    // Falling back *from* local would mean falling back *to* cloud, which is
    // the one thing this module exists to prevent. Nothing is tried.
    if !identity.leaves_the_machine() {
        return Err(failure);
    }

    let Some(local) = local_fallback else {
        return Err(failure);
    };
    debug_assert!(
        !local.identity().leaves_the_machine(),
        "the fallback must be local; this is the whole point of the module"
    );

    let text = local.generate(request, cancel)?;
    Ok(Outcome {
        text,
        used: local.identity(),
        fell_back_from: Some(identity.label()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::fake::Failure;
    use crate::summary::fake::FakeBackend;

    fn request() -> Request {
        Request {
            system: "rules".into(),
            user: "<transcript>hello</transcript>".into(),
        }
    }

    fn every_failure() -> [Failure; 4] {
        // The four shapes ticket 07 names: refused connection, 401, timeout
        // mid-stream, malformed response. A fallback that only handles the
        // one its author imagined is the one that will not fire.
        [
            Failure::Unreachable,
            Failure::Refused,
            Failure::TimedOut,
            Failure::Malformed,
        ]
    }

    #[test]
    fn a_fresh_install_has_not_chosen_and_says_so() {
        // ADR-0013: no preselection. A `Choice` with a `Default` would be a
        // choice made by accident.
        let knob = Knob::default();
        assert!(!knob.is_configured());
        assert_eq!(knob.choice, None);
    }

    #[test]
    fn cloud_cannot_be_chosen_before_the_warning_is_accepted() {
        // Story 36. A gate a Client can walk around by forgetting to call
        // it is not a gate, so the refusal lives here rather than in the UI.
        let mut knob = Knob::default();
        assert!(
            knob.choose(Choice::Cloud {
                provider: "OpenAI".into()
            })
            .is_err()
        );
        assert!(!knob.is_configured(), "and nothing was set");

        knob.cloud_warning_accepted = true;
        assert!(
            knob.choose(Choice::Cloud {
                provider: "OpenAI".into()
            })
            .is_ok()
        );
    }

    #[test]
    fn local_never_falls_back_to_anything() {
        // The property this module exists for, driven through every failure
        // shape. There is no cloud Backend in this test *to* fall back to —
        // and there is no parameter that could carry one.
        for failure in every_failure() {
            let mut local = FakeBackend::failing(failure);
            let mut other = FakeBackend::returning("should never run");
            let knob = Knob {
                choice: Some(Choice::Local),
                ..Knob::default()
            };

            let result = run(
                &knob,
                &mut local,
                Some(&mut other),
                &request(),
                &Cancel::new(),
            );
            assert!(result.is_err(), "{failure:?} should surface, not reroute");
            assert_eq!(
                other.calls(),
                0,
                "{failure:?}: nothing else was tried after local failed"
            );
        }
    }

    #[test]
    fn cloud_falls_back_to_local_on_every_failure_shape() {
        for failure in every_failure() {
            let mut cloud = FakeBackend::cloud(
                "OpenAI",
                vec![crate::summary::fake::Response::Fails(failure)],
            );
            let mut local = FakeBackend::returning("# Local summary");
            let knob = Knob {
                choice: Some(Choice::Cloud {
                    provider: "OpenAI".into(),
                }),
                cloud_warning_accepted: true,
                ..Knob::default()
            };

            let outcome = run(
                &knob,
                &mut cloud,
                Some(&mut local),
                &request(),
                &Cancel::new(),
            )
            .unwrap_or_else(|error| panic!("{failure:?} should fall back, got {error}"));

            assert_eq!(outcome.text, "# Local summary");
            assert!(!outcome.used.leaves_the_machine());
            assert_eq!(
                outcome.fell_back_from.as_deref(),
                Some("OpenAI (fake)"),
                "{failure:?}: the Operator is told why quality changed"
            );
        }
    }

    #[test]
    fn strict_mode_never_switches_even_in_the_permitted_direction() {
        // Story 39: resilience traded for predictability, on purpose.
        for failure in every_failure() {
            let mut cloud = FakeBackend::cloud(
                "OpenAI",
                vec![crate::summary::fake::Response::Fails(failure)],
            );
            let mut local = FakeBackend::returning("# Local summary");
            let knob = Knob {
                choice: Some(Choice::Cloud {
                    provider: "OpenAI".into(),
                }),
                strict: true,
                cloud_warning_accepted: true,
            };

            assert!(
                run(
                    &knob,
                    &mut cloud,
                    Some(&mut local),
                    &request(),
                    &Cancel::new()
                )
                .is_err(),
                "{failure:?} should be reported, not routed around"
            );
            assert_eq!(local.calls(), 0, "{failure:?}: nothing switched");
        }
    }

    #[test]
    fn cancelling_a_cloud_generation_does_not_run_it_locally_instead() {
        // An Operator who pressed stop must not find that stopping caused a
        // second generation. Cancellation is them succeeding, not the
        // machinery failing.
        let cancel = Cancel::new();
        cancel.cancel();
        let mut cloud = FakeBackend::cloud(
            "OpenAI",
            vec![crate::summary::fake::Response::Text("never".into())],
        );
        let mut local = FakeBackend::returning("# Local");
        let knob = Knob {
            choice: Some(Choice::Cloud {
                provider: "OpenAI".into(),
            }),
            cloud_warning_accepted: true,
            ..Knob::default()
        };

        let result = run(&knob, &mut cloud, Some(&mut local), &request(), &cancel);
        assert!(matches!(result, Err(BackendError::Cancelled)));
        assert_eq!(local.calls(), 0);
    }

    #[test]
    fn a_successful_cloud_run_is_not_reported_as_a_fallback() {
        let mut cloud = FakeBackend::cloud(
            "Anthropic",
            vec![crate::summary::fake::Response::Text(
                "# Cloud summary".into(),
            )],
        );
        let knob = Knob {
            choice: Some(Choice::Cloud {
                provider: "Anthropic".into(),
            }),
            cloud_warning_accepted: true,
            ..Knob::default()
        };
        let outcome = run(&knob, &mut cloud, None, &request(), &Cancel::new()).expect("runs");
        assert_eq!(outcome.fell_back_from, None);
        assert!(outcome.used.leaves_the_machine());
    }

    #[test]
    fn cloud_with_no_local_available_reports_the_failure() {
        // A machine with no local model. Better to say so than to retry the
        // cloud forever or pretend it worked.
        let mut cloud = FakeBackend::cloud(
            "OpenAI",
            vec![crate::summary::fake::Response::Fails(Failure::Unreachable)],
        );
        let knob = Knob {
            choice: Some(Choice::Cloud {
                provider: "OpenAI".into(),
            }),
            cloud_warning_accepted: true,
            ..Knob::default()
        };
        assert!(run(&knob, &mut cloud, None, &request(), &Cancel::new()).is_err());
    }

    #[test]
    fn the_active_backend_is_reported_not_the_configured_one() {
        // Story 38's actual requirement. After a fallback these differ, and
        // showing the configured one would tell the Operator their data went
        // somewhere it did not — or, worse, the reverse.
        let mut cloud = FakeBackend::cloud(
            "OpenAI",
            vec![crate::summary::fake::Response::Fails(Failure::TimedOut)],
        );
        let mut local = FakeBackend::returning("# Local");
        let knob = Knob {
            choice: Some(Choice::Cloud {
                provider: "OpenAI".into(),
            }),
            cloud_warning_accepted: true,
            ..Knob::default()
        };
        let outcome = run(
            &knob,
            &mut cloud,
            Some(&mut local),
            &request(),
            &Cancel::new(),
        )
        .expect("falls back");
        assert_eq!(outcome.used.label(), "Local (fake)");
    }
}
