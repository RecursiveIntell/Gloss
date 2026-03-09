//! Parse diagnostics and telemetry for output parsing.
//!
//! [`ParseDiagnostics`] records what happened during output parsing — which
//! strategy was used, whether parsing succeeded, how many retries were
//! attempted, and whether repair or auto-completion was involved.

use llm_output_parser::ParseTrace;
use stack_ids::{AttemptId, TrialId};

/// Records what happened during output parsing.
///
/// Attached to every [`PayloadOutput`](crate::payload::PayloadOutput) produced
/// by [`LlmCall`](crate::llm_call::LlmCall). Tells the caller which extraction
/// strategy succeeded (or that parsing failed), how many retries were attempted,
/// and whether auto-completion or repair was involved.
///
/// # Example
///
/// ```
/// use llm_pipeline::diagnostics::ParseDiagnostics;
///
/// let diag = ParseDiagnostics::default();
/// assert!(diag.ok()); // No parse_error means success
/// ```
#[derive(Debug, Clone, Default)]
pub struct ParseDiagnostics {
    /// Which parse strategy ultimately produced the Value.
    /// e.g. `"lossy"`, `"json"`, `"string_list"`, `"xml_tag"`, `"custom"`.
    pub strategy: Option<&'static str>,

    /// If parsing failed, the error message. `None` means success.
    pub parse_error: Option<String>,

    /// Number of semantic retry attempts (0 = no retries, initial call succeeded
    /// or retry was not configured).
    pub retry_attempts: u32,

    /// Number of transport retries (429, 5xx) before the request succeeded.
    /// 0 = first attempt succeeded.
    pub transport_retries: u32,

    /// Total time spent in backoff delays (milliseconds).
    pub backoff_total_ms: u64,

    /// Whether JSON repair was applied (trailing commas, single quotes, etc.).
    pub repaired: bool,

    /// Whether auto-completion was used (streaming partial parse).
    pub auto_completed: bool,

    /// Parser-level strategies that were attempted before a result was produced.
    pub parser_strategies: Vec<&'static str>,

    /// Specific repair actions reported by the parser.
    pub parser_repair_actions: Vec<String>,

    /// Byte span of the extracted content within the cleaned parser input.
    pub parser_extracted_span: Option<(usize, usize)>,

    /// Non-fatal parser warnings collected during extraction.
    pub parser_warnings: Vec<String>,

    /// Logical retry family identifier from `stack_ids`.
    ///
    /// One `AttemptId` per logical retry family. Generated when retry is
    /// configured and at least one retry attempt occurs. `None` when no
    /// retries are configured or the initial call succeeds without retries.
    pub attempt_id: Option<AttemptId>,

    /// Concrete execution identifier within the retry family.
    ///
    /// Each retry attempt creates a new `TrialId` under the same `AttemptId`.
    /// This field holds the `TrialId` of the *final* attempt (whether it
    /// succeeded or exhausted retries). `None` when no retries occurred.
    pub trial_id: Option<TrialId>,
}

impl ParseDiagnostics {
    /// Quick check: did parsing succeed?
    pub fn ok(&self) -> bool {
        self.parse_error.is_none()
    }

    /// Merge parser trace details into these diagnostics.
    pub fn apply_trace(&mut self, trace: ParseTrace) {
        self.repaired = trace.repaired;
        self.parser_strategies = trace.strategies_tried;
        self.parser_repair_actions = trace.repair_actions;
        self.parser_extracted_span = trace.extracted_span;
        self.parser_warnings = trace.warnings;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diagnostics_default_is_ok() {
        let d = ParseDiagnostics::default();
        assert!(d.ok());
        assert!(d.strategy.is_none());
        assert_eq!(d.retry_attempts, 0);
        assert_eq!(d.transport_retries, 0);
        assert_eq!(d.backoff_total_ms, 0);
        assert!(!d.repaired);
        assert!(!d.auto_completed);
        assert!(d.parser_strategies.is_empty());
        assert!(d.parser_repair_actions.is_empty());
        assert!(d.parser_extracted_span.is_none());
        assert!(d.parser_warnings.is_empty());
        assert!(d.attempt_id.is_none());
        assert!(d.trial_id.is_none());
    }

    #[test]
    fn test_diagnostics_with_error_is_not_ok() {
        let d = ParseDiagnostics {
            parse_error: Some("bad json".to_string()),
            ..Default::default()
        };
        assert!(!d.ok());
    }

    #[test]
    fn test_apply_trace_copies_parser_details() {
        let mut d = ParseDiagnostics::default();
        d.apply_trace(ParseTrace {
            strategies_tried: vec!["direct_parse", "repair_candidate"],
            repaired: true,
            repair_actions: vec!["repaired_candidate".to_string()],
            extracted_span: Some((2, 8)),
            warnings: vec!["fallback path".to_string()],
        });

        assert!(d.repaired);
        assert_eq!(d.parser_strategies, vec!["direct_parse", "repair_candidate"]);
        assert_eq!(d.parser_repair_actions, vec!["repaired_candidate"]);
        assert_eq!(d.parser_extracted_span, Some((2, 8)));
        assert_eq!(d.parser_warnings, vec!["fallback path"]);
    }

    #[test]
    fn test_diagnostics_retry_ids() {
        let d = ParseDiagnostics {
            attempt_id: Some(AttemptId::generate()),
            trial_id: Some(TrialId::generate()),
            ..Default::default()
        };
        assert!(d.attempt_id.is_some());
        assert!(d.trial_id.is_some());
    }
}
