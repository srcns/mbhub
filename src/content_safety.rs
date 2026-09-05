//! Content Safety Filter — deterministic client-side screening.
//!
//! This is the **stage-1 deterministic pre-filter** of the two-stage client-side
//! content safety architecture. It runs in microseconds with zero external calls
//! and catches the high-confidence, *action-oriented* cases that must never enter
//! the gossip layer:
//!
//! - Step-by-step synthesis/manufacture instructions (explosives, precursors, toxins)
//! - Operational attack planning (terrorism, mass violence)
//! - Facilitation of child sexual abuse material (CSAM)
//! - Scheduled-drug production instructions
//!
//! Scope discipline (§7.4): the patterns deliberately require an **action +
//! target** pair, and the action verbs are restricted to imperative/instruction
//! framings ("how to", "step by step", "recipe for", active "-ing" forms).
//! Informational, historical, medical, journalistic or educational statements
//! ("history of methamphetamine", "harm reduction for fentanyl",
//! "how ANFO is used in commercial mining") do NOT match. Disambiguation of the
//! remaining ambiguous minority is the job of stage-2 (contextual classification,
//! §7.3) which the caller runs only on the sender side, fail-closed.
//!
//! Usage:
//! - **Sender-side gate** (before DB write / gossip announce): `screen_text(...)`.
//! - **Receiver-side gate** (before storing/rendering swarm content):
//!   `screen_text(...)` — deterministic only; the receiver never triggers paid
//!   classification on attacker-controlled input (credit-exhaustion DoS).

use regex::Regex;
use std::sync::LazyLock;

/// Outcome of the deterministic safety screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyVerdict {
    /// No high-confidence pattern matched.
    Allow,
    /// A high-confidence action-oriented pattern matched.
    Reject { category: &'static str },
}

impl SafetyVerdict {
    pub fn is_allowed(self) -> bool {
        matches!(self, SafetyVerdict::Allow)
    }
}

/// (regex, category label) pairs. First match wins; each pattern is an
/// *action + target* pair, never a bare keyword.
static PATTERNS: LazyLock<Vec<(&'static Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (
            &*RE_CSAM_FACILITATION,
            "Child exploitation facilitation",
        ),
        (
            &*RE_EXPLOSIVE_MANUFACTURE,
            "Explosive manufacture instructions",
        ),
        (
            &*RE_TOXIN_SYNTHESIS,
            "Chemical weapon / toxin synthesis",
        ),
        (
            &*RE_DRUG_PRODUCTION,
            "Illicit drug production instructions",
        ),
        (
            &*RE_ATTACK_PLANNING,
            "Operational attack planning",
        ),
        (
            &*RE_TR_FACILITATION,
            "TR: Illicit manufacture / exploitation facilitation",
        ),
    ]
});

// CSAM facilitation: creation/acquisition/distribution intent near CSAM terms.
static RE_CSAM_FACILITATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?ix)
        (?:
            (?:how\s+to|where\s+to|create|produce|generate|make|find|get|obtain|share|trade|distribute|sell|access|download)\s{0,30}
            (?:child\s+(?:porn|sexual\s+abuse\s+material)|csam|c\.?p\.?\s*(?:videos|content)|loli(?:con)?|underage\s+(?:porn|nudes|sexual))
        |
            (?:child\s+(?:porn|sexual\s+abuse\s+material)|csam)\s{0,30}
            (?:how\s+to|where\s+to|create|produce|generate|make|find|share|trade|distribute|sell|access)
        )"#,
    )
    .unwrap()
});

// Explosive / IED / precursor manufacture: instruction framing + target.
static RE_EXPLOSIVE_MANUFACTURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?ix)
        (?:
            (?:how\s+to|step[- ]by[- ]step|instructions?\s+(?:for|to)|recipe\s+for)\s{0,30}
            (?:make|build|construct|manufacture|assemble|prepare|synthesize|cook)\s{0,30}
            (?:a\s+|an\s+)?
            (?:pipe\s+bomb|pressure\s+cooker\s+bomb|car\s+bomb|bomb|explosive\s+device|ied|improvised\s+explosive|detonator|blasting\s+cap)
        |
            (?:synthesiz|manufactur)(?:ing|e|es)\s{0,30}
            (?:nitroglycerin|tatp|hmt|rdx|petn|anfo|mercury\s+fulminate|nitroglycol|c-?4|semtex|plastic\s+explosive|picric\s+acid)
        |
            (?:bomb|explosive)\s+(?:making|recipe|ingredients|instructions)
        |
            (?:recipe|instructions?)\s+(?:for|to)\s{0,30}
            (?:nitroglycerin|tatp|hmt|rdx|petn|anfo|mercury\s+fulminate|nitroglycol|c-?4|semtex|plastic\s+explosive|picric\s+acid)
        )"#,
    )
    .unwrap()
});

// Chemical weapon / poison synthesis.
static RE_TOXIN_SYNTHESIS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?ix)
        (?:
            (?:how\s+to|step[- ]by[- ]step|instructions?\s+(?:for|to)|recipe\s+for)\s{0,30}
            (?:synthesiz|manufactur|produce|prepare|make)(?:ing|e|es)?\s{0,30}
            (?:sarin|tabun|soman|vx\s+gas|ricin|nerve\s+agent|mustard\s+gas|novichok|abrin)
        |
            (?:synthesiz|manufactur)(?:ing|e|es)\s{0,30}
            (?:sarin|tabun|soman|ricin|nerve\s+agent|mustard\s+gas|novichok|abrin)
        )"#,
    )
    .unwrap()
});

// Scheduled-drug production (instruction framing + cook/synthesize + substance).
static RE_DRUG_PRODUCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?ix)
        (?:
            (?:how\s+to|step[- ]by[- ]step|recipe\s+for|instructions?\s+(?:for|to))?\s{0,30}
            (?:synthesiz|manufactur|cook|brew)(?:ing|e|es)?\s{0,30}
            (?:methamphetamine|crystal\s+meth|\bmeth\b|fentanyl|carfentanil|crack\s+cocaine|cocaine\s+base|heroin|\blsd\b|\bmdma\b|\bghb\b)
        )"#,
    )
    .unwrap()
});

// Operational attack planning (terrorism / mass violence).
static RE_ATTACK_PLANNING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?ix)
        (?:
            how\s+to\s+(?:plan|execute|carry\s+out|conduct|organize)\s{0,30}
            (?:a\s+)?(?:terrorist|terror|mass\s+shooting|bombing|assassination|suicide\s+bomb)\s*(?:attack)?
        |
            (?:terrorist|terror)\s+attack\s+(?:planning|tutorial|guide|instructions)
        |
            (?:target\s+selection|attack\s+planning|weapon\s+selection)\s+for\s+(?:terror|mass\s+casualty|active\s+shooter)
        )"#,
    )
    .unwrap()
});

// Turkish action-oriented patterns (bomb/explosive/drug manufacture + CSAM).
static RE_TR_FACILITATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?ix)
        (?:
            (?:nasıl|nasil)\s+(?:yapılır|yapilir|yapabilirim|üretilir|uretilir|imal\s+edilir)\s{0,30}
            (?:bomba|patlayıcı|patlayici|metamfetamin|crystal\s+meth|eroin|tnt)
        |
            (?:bomba|patlayıcı|patlayici|metamfetamin|crystal\s+meth|eroin)\s+(?:nasıl|nasil)\s+(?:yapılır|yapilir|üretilir|uretilir)
        |
            (?:çocuk|cocuk)\s+(?:pornosu|pornografisi)\s+(?:nasıl|nasil)\s+(?:bulunur|üretilir|uretilir|indirilir)
        )"#,
    )
    .unwrap()
});

/// Screens `text` against the deterministic high-confidence pattern set.
///
/// O(n) single pass over the compiled regex list; microsecond-scale latency
/// for 64 KB payloads. No external calls, no state.
pub fn screen_text(text: &str) -> SafetyVerdict {
    for (re, category) in PATTERNS.iter() {
        if re.is_match(text) {
            return SafetyVerdict::Reject { category };
        }
    }
    SafetyVerdict::Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_clean_technical_content() {
        assert_eq!(screen_text("How does Rust ownership work?"), SafetyVerdict::Allow);
        assert_eq!(
            screen_text("Explain BLAKE3 hashing in distributed systems."),
            SafetyVerdict::Allow
        );
        assert_eq!(
            screen_text("# SimHash\n\nSimilarity estimation via Hamming distance."),
            SafetyVerdict::Allow
        );
    }

    #[test]
    fn allows_educational_and_historical_content() {
        // §7.4 — informational/discussion content must pass.
        assert_eq!(
            screen_text("History of methamphetamine regulation in the US."),
            SafetyVerdict::Allow
        );
        assert_eq!(
            screen_text("Harm reduction strategies for fentanyl overdose."),
            SafetyVerdict::Allow
        );
        assert_eq!(
            screen_text("How ANFO is used in commercial mining blasting."),
            SafetyVerdict::Allow
        );
        assert_eq!(
            screen_text("Children's online safety and abuse prevention education."),
            SafetyVerdict::Allow
        );
        // Past-tense descriptions are not actionable instructions.
        assert_eq!(
            screen_text("The 1995 attack was planned by a terrorist cell."),
            SafetyVerdict::Allow
        );
    }

    #[test]
    fn rejects_explosive_manufacture_instructions() {
        assert_eq!(
            screen_text("How to make a pipe bomb with household items"),
            SafetyVerdict::Reject {
                category: "Explosive manufacture instructions"
            }
        );
        assert_eq!(
            screen_text("Step by step recipe for TATP synthesis"),
            SafetyVerdict::Reject {
                category: "Explosive manufacture instructions"
            }
        );
        assert_eq!(
            screen_text("Synthesizing ANFO for detonation"),
            SafetyVerdict::Reject {
                category: "Explosive manufacture instructions"
            }
        );
    }

    #[test]
    fn rejects_toxin_synthesis() {
        assert_eq!(
            screen_text("How to synthesize ricin at home"),
            SafetyVerdict::Reject {
                category: "Chemical weapon / toxin synthesis"
            }
        );
    }

    #[test]
    fn rejects_drug_production() {
        assert_eq!(
            screen_text("How to cook crystal meth"),
            SafetyVerdict::Reject {
                category: "Illicit drug production instructions"
            }
        );
        assert_eq!(
            screen_text("Synthesize fentanyl instructions"),
            SafetyVerdict::Reject {
                category: "Illicit drug production instructions"
            }
        );
    }

    #[test]
    fn rejects_csam_facilitation() {
        assert_eq!(
            screen_text("Where to find CSAM"),
            SafetyVerdict::Reject {
                category: "Child exploitation facilitation"
            }
        );
        assert_eq!(
            screen_text("Generate child sexual abuse material"),
            SafetyVerdict::Reject {
                category: "Child exploitation facilitation"
            }
        );
    }

    #[test]
    fn rejects_attack_planning() {
        assert_eq!(
            screen_text("How to plan a mass shooting attack"),
            SafetyVerdict::Reject {
                category: "Operational attack planning"
            }
        );
    }

    #[test]
    fn rejects_turkish_action_patterns() {
        assert_eq!(
            screen_text("Evde bomba nasıl yapılır adım adım"),
            SafetyVerdict::Reject {
                category: "TR: Illicit manufacture / exploitation facilitation"
            }
        );
    }

    #[test]
    fn bare_keywords_without_action_do_not_match() {
        assert_eq!(screen_text("nitroglycerin"), SafetyVerdict::Allow);
        assert_eq!(screen_text("methamphetamine"), SafetyVerdict::Allow);
        assert_eq!(screen_text("fentanyl"), SafetyVerdict::Allow);
    }

    #[test]
    fn case_insensitive_matching() {
        assert_eq!(
            screen_text("HOW TO MAKE A BOMB"),
            SafetyVerdict::Reject {
                category: "Explosive manufacture instructions"
            }
        );
    }

    #[test]
    fn verdict_helper_is_allowed() {
        assert!(SafetyVerdict::Allow.is_allowed());
        assert!(!SafetyVerdict::Reject { category: "x" }.is_allowed());
    }
}
