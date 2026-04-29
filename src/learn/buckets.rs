//! Categorize each fetched [`ReviewThread`] into the bucket that learn cares
//! about (blick-resolved, blick-unresolved, blick-outdated, human-authored).

use serde::Serialize;

use super::threads::ReviewThread;

/// Substring that uniquely identifies blick-authored review-thread comments.
/// Every line comment blick posts (see [`crate::render`]) ends with a footer
/// linking back to the project, which makes for a more reliable filter than
/// a per-deployment bot login.
const BLICK_COMMENT_SIGNATURE: &str = "[Blick](https://github.com/tuist/blick)";

#[derive(Debug, Clone, Copy, Serialize)]
pub(super) enum Bucket {
    /// Thread opened by blick that a human resolved (suggestion accepted).
    BlickResolved,
    /// Thread opened by blick that was left unresolved on a merged PR
    /// (suggestion likely ignored).
    BlickUnresolved,
    /// Thread opened by blick that GitHub flagged as outdated (ambiguous).
    BlickOutdated,
    /// Thread opened by a human reviewer — strong signal of either
    /// something blick missed, or a class of feedback humans care about
    /// that the current skills don't cover.
    HumanAuthored,
}

pub(super) fn bucket_for(thread: &ReviewThread) -> Option<Bucket> {
    let first = thread.first_body();
    if first.contains(BLICK_COMMENT_SIGNATURE) {
        if thread.is_outdated {
            return Some(Bucket::BlickOutdated);
        }
        return Some(if thread.is_resolved {
            Bucket::BlickResolved
        } else {
            Bucket::BlickUnresolved
        });
    }
    // Treat anything authored by a real user as human feedback. Bots that
    // aren't blick (other automations) are ignored — their patterns aren't
    // human review signal and would just add noise.
    if thread.first_author_type() == "User" {
        return Some(Bucket::HumanAuthored);
    }
    None
}

#[derive(Debug, Default, Serialize)]
pub(super) struct ThreadBuckets {
    pub(super) blick_resolved: Vec<ReviewThread>,
    pub(super) blick_unresolved: Vec<ReviewThread>,
    pub(super) blick_outdated: Vec<ReviewThread>,
    pub(super) human_authored: Vec<ReviewThread>,
}

impl ThreadBuckets {
    pub(super) fn push(&mut self, bucket: Bucket, thread: ReviewThread) {
        match bucket {
            Bucket::BlickResolved => self.blick_resolved.push(thread),
            Bucket::BlickUnresolved => self.blick_unresolved.push(thread),
            Bucket::BlickOutdated => self.blick_outdated.push(thread),
            Bucket::HumanAuthored => self.human_authored.push(thread),
        }
    }

    pub(super) fn total(&self) -> usize {
        self.blick_resolved.len()
            + self.blick_unresolved.len()
            + self.blick_outdated.len()
            + self.human_authored.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::learn::threads::ThreadComment;

    fn comment(author_type: &str, body: &str) -> ThreadComment {
        ThreadComment {
            author: "alice".into(),
            author_type: author_type.into(),
            body: body.into(),
        }
    }

    fn thread(comments: Vec<ThreadComment>, resolved: bool, outdated: bool) -> ReviewThread {
        ReviewThread {
            pr_number: 1,
            is_resolved: resolved,
            is_outdated: outdated,
            path: Some("src/lib.rs".into()),
            line: Some(1),
            url: Some("https://example/x".into()),
            comments,
        }
    }

    #[test]
    fn bucket_blick_resolved_and_unresolved() {
        let blick_body = "**foo** body — [Blick](https://github.com/tuist/blick) · default review";
        let resolved = thread(vec![comment("Bot", blick_body)], true, false);
        assert!(matches!(bucket_for(&resolved), Some(Bucket::BlickResolved)));

        let unresolved = thread(vec![comment("Bot", blick_body)], false, false);
        assert!(matches!(
            bucket_for(&unresolved),
            Some(Bucket::BlickUnresolved)
        ));
    }

    #[test]
    fn bucket_human_authored_thread() {
        let human = thread(
            vec![comment(
                "User",
                "We never validate this input — looks like a bug.",
            )],
            false,
            false,
        );
        assert!(matches!(bucket_for(&human), Some(Bucket::HumanAuthored)));
    }

    #[test]
    fn other_bot_threads_are_skipped() {
        let dependabot = thread(
            vec![comment("Bot", "Bumps foo from 1.0 to 1.1")],
            false,
            false,
        );
        assert!(bucket_for(&dependabot).is_none());
    }

    #[test]
    fn outdated_blick_thread_buckets_as_outdated_regardless_of_resolved_state() {
        let blick_body = "x [Blick](https://github.com/tuist/blick) y";
        let mut t = thread(vec![comment("Bot", blick_body)], true, true);
        assert!(matches!(bucket_for(&t), Some(Bucket::BlickOutdated)));
        t.is_outdated = false;
        t.is_resolved = false;
        assert!(matches!(bucket_for(&t), Some(Bucket::BlickUnresolved)));
    }

    #[test]
    fn empty_thread_with_no_comments_is_ignored() {
        let empty = thread(vec![], false, false);
        assert!(bucket_for(&empty).is_none());
    }

    #[test]
    fn thread_buckets_total_sums_all_four_lanes() {
        let mut b = ThreadBuckets::default();
        let t = thread(
            vec![comment("Bot", "[Blick](https://github.com/tuist/blick)")],
            true,
            false,
        );
        b.push(Bucket::BlickResolved, t.clone());
        b.push(Bucket::BlickUnresolved, t.clone());
        b.push(Bucket::BlickOutdated, t.clone());
        b.push(Bucket::HumanAuthored, t);
        assert_eq!(b.total(), 4);
    }
}
