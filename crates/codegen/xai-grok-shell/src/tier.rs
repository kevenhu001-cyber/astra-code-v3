//! Subscription-tier classification shared across the shell and the pager.
//!
//! The subscription tier reaches the client as a free-form display-name string.
//! Keep the classification in one place so command and tool availability do
//! not diverge.

/// Whether a known tier is restricted for personal-account features.
/// Unknown future tiers fail open; an empty tier is treated as free.
pub fn is_restricted_tier_name(tier: &str) -> bool {
    let tier = tier.trim().to_ascii_lowercase();
    tier.is_empty() || tier == "free" || tier == "x basic" || tier == "x_basic"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_restricted_personal_tiers() {
        assert!(is_restricted_tier_name(""));
        assert!(is_restricted_tier_name("Free"));
        assert!(is_restricted_tier_name("X Basic"));
        assert!(is_restricted_tier_name("x_basic"));
        assert!(!is_restricted_tier_name("SuperGrok"));
        assert!(!is_restricted_tier_name("X Premium"));
        assert!(!is_restricted_tier_name("some_new_plan"));
    }
}
