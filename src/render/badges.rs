//! Visual severity indicators rendered into review output.
//!
//! Two flavors:
//! - shields.io priority badges for human-facing markdown (`P1` red, etc.)
//! - GitHub Check Run annotation levels (`failure`/`warning`/`notice`)

use crate::config::Severity;

/// Codex-style shields.io priority badge. `<sub><sub>` shrinks the badge so
/// it sits inline with text without dominating the line.
pub fn severity_badge(severity: Severity) -> String {
    let (priority, color) = match severity {
        Severity::High => ("P1", "red"),
        Severity::Medium => ("P2", "yellow"),
        Severity::Low => ("P3", "green"),
    };
    format!(
        "<sub><sub>![{priority} Badge](https://img.shields.io/badge/{priority}-{color}?style=flat)</sub></sub>"
    )
}

/// Map a `Severity` to a GitHub Check Run annotation level. The Checks API
/// only accepts these three string values.
pub fn severity_to_annotation_level(severity: Severity) -> &'static str {
    match severity {
        Severity::High => "failure",
        Severity::Medium => "warning",
        Severity::Low => "notice",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_severity_is_red_p1_failure() {
        assert!(severity_badge(Severity::High).contains("P1-red"));
        assert_eq!(severity_to_annotation_level(Severity::High), "failure");
    }

    #[test]
    fn medium_severity_is_yellow_p2_warning() {
        assert!(severity_badge(Severity::Medium).contains("P2-yellow"));
        assert_eq!(severity_to_annotation_level(Severity::Medium), "warning");
    }

    #[test]
    fn low_severity_is_green_p3_notice() {
        assert!(severity_badge(Severity::Low).contains("P3-green"));
        assert_eq!(severity_to_annotation_level(Severity::Low), "notice");
    }

    #[test]
    fn badge_is_wrapped_in_double_sub_for_inline_sizing() {
        // The double-`<sub>` wrapping is load-bearing — without it the badge
        // image dominates surrounding text. Lock the structure in.
        let badge = severity_badge(Severity::High);
        assert!(badge.starts_with("<sub><sub>"));
        assert!(badge.ends_with("</sub></sub>"));
    }
}
