//! Session state machine for OFA. Pure logic with no I/O, so it can be
//! unit-tested against recorded events. Filled in during milestone 3.

/// What the island shows, most urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IslandState {
    NeedsYou,
    Failed,
    Working,
    Done,
    Idle,
}

impl IslandState {
    /// The state the island shows for a set of sessions: the most urgent one.
    pub fn most_urgent(states: impl IntoIterator<Item = IslandState>) -> IslandState {
        states.into_iter().min().unwrap_or(IslandState::Idle)
    }
}

#[cfg(test)]
mod tests {
    use super::IslandState::*;
    use super::*;

    #[test]
    fn empty_is_idle() {
        assert_eq!(IslandState::most_urgent([]), Idle);
    }

    #[test]
    fn needs_you_beats_everything() {
        assert_eq!(
            IslandState::most_urgent([Done, Working, NeedsYou, Failed]),
            NeedsYou
        );
        assert_eq!(IslandState::most_urgent([Done, Working, Failed]), Failed);
        assert_eq!(IslandState::most_urgent([Idle, Done, Working]), Working);
    }
}
